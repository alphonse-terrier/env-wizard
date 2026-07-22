# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-07-22

### Added
- Code scanning now also detects NestJS `@nestjs/config` (`configService.get("FOO")`,
  `.get<Type>("FOO")`, `.getOrThrow("FOO")`) and Zod-style env schemas (zod / t3-env /
  znv), e.g. `FOO: z.string()` — any `SCREAMING_SNAKE_CASE` key mapped to a zod
  validator, including multi-line chained validators.

## [0.5.0] - 2026-07-21

### Added
- After a normal (non-`--from-code`, non-`--yes`) run finishes writing `.env`,
  env-wizard now offers to also add any variables it can see are used in the
  code but weren't just written, prompting for their values the same guided
  way and appending them to the file. Closes the common drift gap without
  requiring `--from-code` or a separate `env-wizard scan`.
- `env-wizard scan --check` exits with status 1 if any drift is found (missing
  or unused variables), so `scan` can gate CI instead of being report-only.
- Code scanning now also detects C# (`Environment.GetEnvironmentVariable`) and
  Java/Kotlin (`System.getenv`), bringing the total to 8 languages.
- Three more AI provider presets in `env-wizard config`'s picker: LM Studio,
  OpenRouter, and Groq — previously these required hand-typing the base URL,
  model, and API key env var via "Other OpenAI-compatible API…".
- `env-wizard completions <shell>` generates a shell completion script (bash,
  zsh, fish, elvish, powershell).

### Fixed
- A failure to restrict a freshly written `.env`'s permissions to `0600` on
  Unix used to fail silently; it now prints a visible warning instead, since
  the file holds secrets.
- HTTP provider errors (bad API key, rate limits, etc.) used to be misreported
  as "endpoint unreachable or timed out" even when the provider responded —
  the actual status code and response body are now surfaced.
- Code scanning no longer flags commented-out access idioms (`// process.env.OLD`,
  `# os.getenv("OLD")`) as real usage.

## [0.4.0] - 2026-07-21

### Added
- Support structured config **templates** as an alternative to `.env.example`:
  `config.example.toml`, `settings.sample.yaml`, `appsettings.example.json` (or a
  `.dist`/`.template` variant) get the same guided walkthrough and produce the real
  file. Only the values you change are touched — comments, key order, and
  indentation are preserved byte-for-byte for everything else. Only scalar fields
  (string/number/bool) are prompted; arrays and untouched nested tables pass
  through as-is. Auto-detected the same way as dotenv aliases (dotenv still takes
  priority when both exist); `--from-code` and `env-wizard scan` remain dotenv-only.
  The format itself (TOML/YAML/JSON) is detected by reading and parsing the
  template's content, not just its filename — the extension is only a
  fallback for when the content isn't decisive (e.g. empty). This means a
  misnamed template (`config.example.json` that's actually TOML) or one with
  no extension at all (`config.example`) is still detected correctly. Dotenv
  filenames are never reinterpreted this way, even if their quoted values also
  happen to parse as valid TOML. A field whose key is duplicated in the source
  document is never offered for editing, since it can't be addressed
  unambiguously on write-back. For YAML, a value written in quoted style
  (`port: "5432"`) is always treated as a string — editing it can no longer
  silently turn it into a number and drop the quotes — and filling in a
  previously-empty value (`host:`) now correctly inserts the `key: value`
  separator instead of producing an invalid `key:value`. For JSON, a replacement
  value is only ever written as a bare number when it's valid JSON number syntax
  (rejecting things like `007`, `+5`, `.5`, or `nan` that Rust would parse but
  JSON wouldn't); anything else falls back to a quoted string so the file always
  stays valid.

## [0.3.1] - 2026-07-21

### Added
- Auto-detect common example filename aliases (`.env.sample`, `.env.dist`,
  `.env.template`, `env.example`) when `--input` isn't passed, before falling back to
  code detection. An explicit `--input` is always honored verbatim.

## [0.3.0] - 2026-07-21

### Added
- Detect environment variables **used in the source code** (JS/TS, Python, Rust, Go, Ruby,
  PHP) as a complement to `.env.example`:
  - `env-wizard scan` — audit report of vars used in code but missing from the example
    (with `file:line`), and vars declared but unused.
  - Fallback — when no `.env.example` exists, derive the variable list from the code.
  - `--from-code` — merge code-discovered variables into the example-driven run.
- `release.sh` — one-command release (bump version + changelog + README, tag, push).

## [0.2.0] - 2026-07-20

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

[Unreleased]: https://github.com/alphonse-terrier/env-wizard/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/alphonse-terrier/env-wizard/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/alphonse-terrier/env-wizard/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/alphonse-terrier/env-wizard/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/alphonse-terrier/env-wizard/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/alphonse-terrier/env-wizard/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/alphonse-terrier/env-wizard/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/alphonse-terrier/env-wizard/releases/tag/v0.1.0
