# Contributing to Fire

Thanks for your interest! Issues, ideas, docs fixes, and pull requests are all welcome.

## Getting started
```bash
git clone https://github.com/gbenm/fire
cd fire
cargo build
cargo test
./target/debug/fire          # run your local build
```
`samples/` and the `*.fire.yml` files at the repo root are handy configs to try changes against.

## Pull requests
- Keep each PR focused on one change, and describe the problem it solves.
- Add or update tests for behavior changes (`cargo test` must pass; CI runs it on Linux and macOS).
- If you change config options or user-facing behavior, update `docs/` and
  `schemas/fire.schema.json` to match.

## Reporting bugs
Open an [issue](https://github.com/gbenm/fire/issues/new/choose) with your Fire version, OS, and the
smallest `fire.yml` that reproduces the problem.

## Questions and ideas
Use [Discussions](https://github.com/gbenm/fire/discussions) for questions, feature ideas, and
sharing your setups.
