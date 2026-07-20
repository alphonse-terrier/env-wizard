# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Ask the AI a **specific question** about a variable: type `? <question>` or
  `/ask <question>` at a prompt (a bare `?` / `/hint` still gives the generic hint).
- Generated `.env` is **self-documenting**: each variable's `.env.example` comment is
  carried over as `#` lines above its `KEY=value`.
- Cross-platform config location via `dirs` (`%APPDATA%` on Windows, Application Support
  on macOS, `~/.config` on Linux), still overridable with `$ENV_WIZARD_CONFIG` /
  `$XDG_CONFIG_HOME`.
- Release automation: GitHub Actions workflow builds macOS (arm64/x86_64) and Linux
  (x86_64 musl) binaries on a tag, publishes the release with `SHA256SUMS`, and can bump
  the Homebrew tap formula.

### Changed
- Repo-context grep now matches **whole words** (so `PORT` no longer matches `SUPPORT`),
  skips lockfiles, and ignores files larger than 256 KB.

### Security
- Generated `.env` (and its `.env.bak`) are written with `0600` permissions on Unix.

### Fixed
- HTTP providers now use bounded connect/overall timeouts, so an unreachable endpoint
  fails fast instead of hanging.
- A provider CLI that closes stdin early no longer fails the hint with a `BrokenPipe` error.

## [0.1.0] - 2026-07-20

### Added
- Interactive walkthrough of a repo's `.env.example`, one variable at a time, showing each
  inline comment as a hint and writing `.env` (with confirmation and a `.env.bak` backup).
- On-demand AI hints (`?`) grounded in repo context (README, config files, code
  occurrences), with existing dotenv secret files excluded from prompts.
- Configurable AI provider — CLI command (Claude, Ollama, custom) or OpenAI-compatible
  HTTP endpoint — chosen on first use and changeable via `env-wizard config`.
- Markdown-to-terminal rendering of AI answers.
- Prebuilt binaries for macOS (arm64/x86_64) and Linux (x86_64 musl); Homebrew tap.

[Unreleased]: https://github.com/alphonse-terrier/env-wizard/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/alphonse-terrier/env-wizard/releases/tag/v0.1.0
