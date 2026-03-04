# Progressive Examples

These examples show how to combine Fire features.

## 1) Quick command with passthrough args
```yaml
commands:
  hello: echo hello world
```
- `fire hello other args` → `echo hello world other args`
- Uses shorthand; remaining args append to the final command.

## 2) Sequenced exec steps
```yaml
greetings:
  commands:
    hello:
      description: "Hello world"
      exec:
        - echo hello
        - echo world
```
- Runs both commands in order. Descriptions show up in completion/help.

## 3) Health check + fallback runner
```yaml
npm:
  check: npm -v
  fallback_runner: docker run --rm -it node:lts-alpine sh
  exec: npm
```
- If `npm -v` fails locally, Fire runs the command via the fallback container.

## 4) Pre-run hook + runner
```yaml
run:
  before: docker compose ps -q front | grep -q . || docker compose up -d front
  exec: compose exec front npm run
```
- Ensures the service is up before executing inside the container.

## 5) Greedy nested commands
```yaml
run:
  exec: npm run
  commands:
    build: npm run clean && npm run build
    start: fire build && npm run start
```
- `fire run build` selects the nested command; other tokens fall back to `npm run <unknown command>`.

## 6) Macros + nested commands
```yaml
utils:
  compute:
    "{service}": ts:getServiceNameById("{1}")
  macros:
    "{{front}}": docker compose exec front
    "{{dynamic}}": docker compose exec {service}
  exec: "{{front}} npm run"
  commands:
    npm-version: "{{front}} npm -v"
    hello: "{{dynamic}} echo Hello"
```
- Macro substitution happens before placeholders. Compute can define dynamic tokens reused by macros and commands.

## 7) Eval with argument spreads
```yaml
computed:
  <<: *arg-config
  eval: py:sayHello("{1}", "{2}", ...{{n}})
```
- `{1}`, `{2}` pick specific args; `...{{n}}` expands the rest as individual string arguments for the runtime, you can use [{{n}}] to get a string[].

## 8) Eval returning command list
```yaml
commands:
  setup:
    eval: py:getSetupCommands("{1}")
```
- If `getSetupCommands` returns `["echo one", "echo two"]`, Fire executes both commands in order.

## 9) Placeholders
```yaml
template:
  <<: *arg-config
  description: "[your name] [your country] ...[other args]"
  exec:
    - "CMD echo Hello {1}, are you from {2}?"
    - "CMD echo and who else is with you? ...{{n}}"
```

## 10) Runtime compute + exec
```yaml
hash:
  <<: *arg-config
  compute:
    "{hash}": ts:makeHash("{1}", "sha256")
  exec: echo "HASH: {hash}"
```
- `{hash}` is computed via the `ts` runtime and then injected into the command.

## 11) Fallback-only runner with directory prep
```yaml
fallback:
  check: exit -1
  before: echo "ignore me please"
  fallback_runner: docker run --rm -it -v .:/mount alpine sh
  exec:
    - cd mount
    - pwd
    - ls
    - cat README.md
```
- Always triggers the fallback runner because `check` fails.
