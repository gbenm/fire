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

## Changelog
`CHANGELOG.md` follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Every user-facing
change adds a line under `## [Unreleased]`, in the same commit or PR, grouped as **Breaking**,
**Added**, **Changed**, **Deprecated**, **Removed** or **Fixed**, with the issue number. Write it
for users: what changed and what to do, not how it was implemented. Tests, refactors and CI-only
changes don't need an entry.

To release `X.Y.Z`, in the release commit rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`,
add a new empty `## [Unreleased]` above it, and update the compare links at the bottom. A breaking
change bumps the major version. The release workflow uses that section as the GitHub release
notes.

## Reporting bugs
Open an [issue](https://github.com/gbenm/fire/issues/new/choose) with your Fire version, OS, and the
smallest `fire.yml` that reproduces the problem.

## Questions and ideas
Use [Discussions](https://github.com/gbenm/fire/discussions) for questions, feature ideas, and
sharing your setups.
