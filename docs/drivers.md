# External CLI Drivers

Drivers let a Fire command forward to an external CLI while a separately installed adapter
provides project-aware completion. Fire does not parse, quote, expand, or otherwise rewrite the
remaining arguments: each input argument is passed to the executable as one OS argument.

## Configure a driver command

The `driver` value is the exact executable name Fire launches:

```yaml
commands:
  compose:
    description: Manage local containers
    driver: docker-compose
```

`fire compose up web` runs `docker-compose` in the command's resolved `dir` and passes `up` and
`web` as two arguments. The executable must be available through `PATH` (or `driver` may be an
explicit executable path). Driver commands support normal Fire scopes, descriptions,
dependencies, sessions, `agents`, and `dir`.

## Install a completion driver

Driver adapters are Git repositories:

```bash
fire cli driver install https://github.com/example/fire-driver-docker-compose
```

Fire clones the repository, reads `driver.yaml`, runs every build step once through `sh`, and
caches the built repository in the platform user data directory under `fire/drivers/<name>`.
Installing the same driver name again is refused so a cached build is never replaced implicitly.

The repository manifest has this shape:

```yaml
name: docker-compose
build:
  - go build -o bin/complete ./cmd/driver
handlers:
  completion:
    exec: bin/complete
```

The name uses ASCII letters, numbers, `-`, `_`, or `.`; `fire` is reserved. A completion
executable must be a relative path inside its repository. Build steps run in the cloned repository,
while handlers run in the Fire command's project working directory.

## Completion protocol

For completion, Fire executes the installed handler and passes every already completed argument
after the Fire command path, unchanged. The unfinished word is used only to filter the returned
candidates. The handler writes a JSON array to stdout:

```json
[
  {
    "value": "up",
    "description": "Create and start containers",
    "children": [
      { "value": "web", "description": "Web service" },
      { "value": "db", "description": "Database service" }
    ]
  },
  { "value": "down", "description": "Stop and remove containers" }
]
```

A handler may return a tree rooted at the external CLI's first argument, or use the received
arguments to return candidates for the current context directly. Invalid JSON, a failing handler,
or a driver that is not installed produces no delegated candidates and does not interrupt the
shell.
