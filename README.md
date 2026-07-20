<div align="center">

# 🧙 env-wizard

### Never guess what goes in a `.env` again.

**env-wizard** walks you through a freshly cloned repo's `.env.example` one variable
at a time — showing the inline docs as you go, and asking **your** AI (cloud *or*
local) for a repo-aware hint the moment you're stuck. Then it writes your `.env`.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust&logoColor=white)
![AI: cloud or local](https://img.shields.io/badge/AI-cloud%20or%20local-8A2BE2)
![Telemetry: none](https://img.shields.io/badge/telemetry-none-brightgreen)
![Platform: macOS · Linux · Windows](https://img.shields.io/badge/platform-macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-lightgrey)

</div>

---

## ⚡ Install

Pick the method for your OS — then run `env-wizard` inside any repo that has a
`.env.example`.

### 🍎 macOS

**Homebrew** (recommended):

```sh
brew tap alphonse-terrier/env-wizard
brew install env-wizard
```

<details>
<summary>Or download a prebuilt binary</summary>

```sh
# Apple Silicon (M1/M2/M3…). For an Intel Mac, swap aarch64 → x86_64.
curl -L https://github.com/alphonse-terrier/env-wizard/releases/download/v0.1.0/env-wizard-v0.1.0-aarch64-apple-darwin.tar.gz | tar xz
sudo mv env-wizard-v0.1.0-aarch64-apple-darwin/env-wizard /usr/local/bin/
```

</details>

### 🐧 Linux

**Homebrew** (if you use it):

```sh
brew tap alphonse-terrier/env-wizard
brew install env-wizard
```

**Prebuilt static binary** (x86_64, no dependencies):

```sh
curl -L https://github.com/alphonse-terrier/env-wizard/releases/download/v0.1.0/env-wizard-v0.1.0-x86_64-unknown-linux-musl.tar.gz | tar xz
sudo mv env-wizard-v0.1.0-x86_64-unknown-linux-musl/env-wizard /usr/local/bin/
```

### 🪟 Windows

Install with Cargo (needs the [Rust toolchain](https://rustup.rs)):

```powershell
cargo install --git https://github.com/alphonse-terrier/env-wizard
```

### 📦 Any OS — with Cargo

Works everywhere Rust runs:

```sh
cargo install --git https://github.com/alphonse-terrier/env-wizard
```

<details>
<summary>Build from a local clone</summary>

```sh
git clone https://github.com/alphonse-terrier/env-wizard
cd env-wizard
cargo install --path .
```

</details>

> **Verify a download** against [`SHA256SUMS`](https://github.com/alphonse-terrier/env-wizard/releases/latest) on the release page.

### First run

```sh
cd my-freshly-cloned-project
env-wizard
```

That's it. 🎉

---

**Jump to:** [Problem](#-the-problem) · [Demo](#-demo) · [Features](#-why-youll-like-it) · [Choose your AI](#-choose-your-ai) · [Usage](#-usage-reference) · [How it works](#-how-it-works) · [FAQ](#-faq)

## 🤔 The problem

You clone a repo. Its `.env.example` has fifteen variables and terse comments. Half
you can guess; the rest send you spelunking through the README, `docker-compose.yml`,
and the source just to learn what `REDIS_TLS_URL` should look like. Twenty minutes
later, you *still* aren't running.

## ✅ The fix

`env-wizard` reads `.env.example`, asks for each variable in turn, shows its comment
as a hint, and — when you type `?` — asks an AI that has **already read your repo**
what to put and how to get it. Then it writes `.env` for you, safely.

## 🎬 Demo

```console
$ env-wizard
env-wizard
At each prompt, type:
  ┃  Enter   ┃  accept the suggested default
  ┃    ?     ┃  ask the AI for a hint
  ┃ (nothing)┃  leave this variable empty
  ┃    q     ┃  quit without saving
Change the AI provider anytime with `env-wizard config`.

  # Secret used to sign session cookies (32+ chars)
? SECRET_KEY › ?

💡 Hint
SECRET_KEY
This signs your session cookies. Generate one with:

    openssl rand -hex 32

 • Must be at least 32 characters
 • Keep it secret — put it in .env, never commit it

? SECRET_KEY › 9f2c8a…                ← you paste the real value
✔ SECRET_KEY · 9f2c8a…
✓ Wrote .env
```

> The `💡 Hint` is your AI's answer, rendered cleanly in the terminal — headings,
> bullets, and commands, with the raw Markdown stripped away.

## 💛 Why you'll like it

- 🧭 **Guided, not guesswork** — every variable's `.env.example` comment shows inline
  as a hint, *before* you ever call the AI.
- 🤖 **Your AI, your rules** — hints come from whatever provider you pick: Claude,
  OpenAI, a local Ollama model, or any OpenAI-compatible endpoint. Nothing is
  hardcoded, and env-wizard stores **no** API keys.
- 🔒 **Local-friendly** — point it at `http://localhost:11434` (Ollama, LM Studio…)
  and your secrets never leave your machine.
- 🧠 **Repo-aware hints** — the AI is fed your README, common config files, and every
  place the variable appears in the code, so its advice is specific — not generic.
- 💾 **Safe writes** — confirms before overwriting an existing `.env`, and keeps a
  `.env.bak`.
- 🪶 **One small binary** — written in Rust. No runtime, no daemon, starts instantly.

## 🧩 Choose your AI

The **first time** you press `?`, env-wizard asks which AI to use and remembers your
choice. Two kinds are supported:

| Kind | What it is | Presets |
| ---- | ---------- | ------- |
| **CLI command** | Pipes the prompt to a local/cloud CLI. Manages its own auth — no keys stored. | Claude (`claude -p`), Ollama (`ollama run <model>`), or any custom command |
| **OpenAI-compatible HTTP** | `base_url` + `model` + an env var for the API key. | OpenAI, local Ollama (`http://localhost:11434/v1`), LM Studio, OpenRouter, Groq… |

### Changing your provider

Two equivalent ways — pick whichever you like:

```sh
env-wizard config          # re-run the interactive picker
```

…or edit the config file by hand. It lives at
`$XDG_CONFIG_HOME/env-wizard/config.toml` (falling back to
`~/.config/env-wizard/config.toml`; override with `$ENV_WIZARD_CONFIG`):

```toml
kind  = "command"          # "command" | "openai"
label = "Claude (CLI)"     # shown while fetching a hint

[command]                  # when kind = "command"
program    = "claude"
args       = ["-p"]
prompt_via = "arg"         # "arg" (append prompt) | "stdin" (pipe prompt)

# [openai]                 # when kind = "openai"
# base_url    = "https://api.openai.com/v1"
# model       = "gpt-4o-mini"
# api_key_env = "OPENAI_API_KEY"   # empty = no auth (e.g. local Ollama)
```

## 📖 Usage reference

Run it at the root of a repo that has a `.env.example`:

```sh
env-wizard
```

**At each prompt:**

| Input          | Effect                                   |
| -------------- | ---------------------------------------- |
| `Enter`        | Accept the shown default                 |
| `?` / `/hint`  | Ask the AI for a hint, then re-prompt    |
| *(empty)*      | Leave the variable empty                 |
| `q`            | Quit without writing                     |

**Options:**

| Flag                  | Description                                          |
| --------------------- | ---------------------------------------------------- |
| `-i, --input <PATH>`  | Example file to read (default `.env.example`)        |
| `-o, --output <PATH>` | Env file to write (default `.env`)                   |
| `-y, --yes`           | Accept all defaults and overwrite without confirming |
| `--no-ai`             | Disable the AI hint feature (no calls to a provider) |

**Commands:**

| Command          | Description                              |
| ---------------- | ---------------------------------------- |
| `env-wizard`     | Run the interactive `.env` filler        |
| `env-wizard config` | Choose or change the AI provider      |

## 🛠 How it works

```
.env.example ──▶ parse ──▶ prompt loop ──▶ .env
                              │
                   type "?" ──┤
                              ▼
              gather repo context (README + configs + grep)
                              ▼
                 your AI provider (CLI or HTTP)
                              ▼
                render the hint cleanly in the terminal
```

## ❓ FAQ

**Does my data leave my machine?**
Only if you choose a cloud provider. Pick a local one (Ollama, LM Studio) and the
prompt never leaves your laptop. env-wizard itself has no telemetry and stores no
API keys — cloud CLIs and endpoints use their own configured credentials.

**Could my existing `.env` secrets be sent to the AI?**
No. When building context for a hint, env-wizard deliberately **skips real dotenv
files** (`.env`, `.env.local`, `.env.production`, …) — only template files like
`.env.example` are ever read. So values already in your `.env` are never included in
a prompt. (See `is_secret_env_file` in `src/hint.rs`.)

**I don't have an AI CLI installed — is env-wizard still useful?**
Yes. The whole guided flow (inline comment hints, defaults, safe writing) works
without any AI. If you press `?` and the chosen provider isn't reachable, you get a
clear error and the wizard simply continues. You can also run with `--no-ai`.

**Which platforms are supported?**
macOS and Linux. (Windows isn't tested yet — contributions welcome.)

## 📦 Requirements

- Nothing extra for the Homebrew or prebuilt-binary installs. A Rust toolchain
  (`cargo`) is only needed for the Cargo / from-source methods.
- For the `?` hint only: the provider you pick must be reachable — the chosen CLI on
  your `PATH`, or the HTTP endpoint up with its API key set. If not, everything else
  still works and the hint reports a clear error.

## 🤝 Contributing

Issues and PRs are welcome! Before opening a PR, please run:

```sh
cargo test
cargo clippy --all-targets
```

## 📄 License

[MIT](LICENSE) © Alphonse Terrier
