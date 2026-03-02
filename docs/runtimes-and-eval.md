# Runtimes, `eval`, and Dynamic Scripts

Fire can evaluate JavaScript/TypeScript (node or deno) and Python snippets directly from your YAML config. Define runtimes once, then reference them from `eval` expressions and `compute` entries.

## Defining runtimes
```yaml
runtimes:
  ts:
    sdk: deno
    runner: deno
    check: deno --version
    fallback_runner: docker run --rm -it denoland/deno:latest
    paths:
      - scripts/*.ts
      - scripts/helpers/*.ts

  js:
    sdk: node
    paths:
      - scripts/*.mjs

  py:
    sdk: python
    runner: python3.13
    paths:
      - scripts/*.py
```
Fields:
- `sdk`: one of `node`, `deno`, `python`.
- `runner`: command to launch the runtime process.
- `check`: optional probe; if it fails, `fallback_runner` is used.
- `fallback_runner`: alternative runner (often a container image) when `check` fails.
- `paths`: glob patterns to pre-load modules/functions into the runtime session.

## `eval` expressions
Syntax: `<runtimeKey>:<code>`
- Runtime key must match `runtimes` (e.g., `py`, `ts`, `js`).
- Placeholders inside code are replaced **only if** the command defines `placeholder`; the pattern controls all forms (`{1}`, `...{{n}}`, `[{{n}}]`).
- `on_unused_args` applies **only** here; default is `ignore`.

Examples:
```yaml
computed:
  <<: *arg-config
  eval: py:sayHello("{1}", "{2}", ...{{n}})

computed2:
  <<: *arg-config
  eval: ts:makeHash("{1}", "sha256")

computed3:
  <<: *arg-config
  eval: js:getCurrentTimestamp()
```

## Library loading and sessions
For each runtime key, Fire starts a session, loads files matching `paths`, and reuses the same process for multiple `eval` statements. Outputs printed by the runtime are forwarded to the user; returned values are printed when present.

## Compute + eval
`compute` entries can also use runtime prefixes. These run **before** command rendering and can feed into placeholders:
```yaml
compute:
  arg1: ts:makeHash("{1}", "sha256")
exec: echo "Hello {1}, your job is {2}"
```

## Error handling
- Invalid runtime key → Fire exits with an error.
- Runtime `check` failure → uses `fallback_runner` if configured; otherwise exits.
- `on_unused_args: error` → aborts before running the runtime if extra args weren’t consumed by placeholders.
