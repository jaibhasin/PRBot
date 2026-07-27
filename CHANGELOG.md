# Keep a changelog for humans.
# Version numbers must match Cargo.toml when cutting a release.

## [Unreleased]

### Added
- Incremental contextual reviews that re-check only bundles affected since the last reviewed head
- File-level GitHub comment fallback for ambiguous or unresolvable anchors
- Definition-oriented `find_symbol` and word-bounded `find_references` tools
- GitHub write retries for review and comment publishing
- `ReviewRun` plus `skipped`/`failed` run outcome statuses
- Richer specialist roles for API, concurrency, and performance signals
- 50-case evaluation fixture catalog skeleton under `evals/fixtures/`

### Changed
- Summary comments report incremental mode and reviewed bundle counts
- Fingerprints for changed paths are cleared on incremental reruns so findings can be revalidated

## [0.1.0] - 2026-07-26

### Added
- Rust CLI scaffold (`prbot version`, `prbot review` stub)
- GitHub Action packaging (`action.yml` + Docker image)
- Example consumer workflow
- CI workflow for format, clippy, tests, and release build
