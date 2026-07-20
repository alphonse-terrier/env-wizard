<div align="center">

# 🧙 env-wizard

**Stop guessing what goes in your `.env`.**

A tiny, fast CLI that walks you through a freshly cloned repo's `.env.example`
one variable at a time — showing the inline docs as you go, and calling *your*
AI (cloud or local) for a hint whenever you're stuck.

</div>

---

## The problem

You clone a repo. There's a `.env.example` with fifteen variables and terse
comments. Half of them you can guess; the other half send you spelunking through
the README, `docker-compose.yml`, and the source just to find out what
`REDIS_TLS_URL` is supposed to look like. Twenty minutes later you're still not
running.

## The fix

```sh
env-wizard
```

env-wizard reads `.env.example`, asks for each variable in turn, shows its
comment as a hint, and — when you type `?` — asks an AI that has *already read
your repo* what to put and how to get it. Then it writes `.env` for you.

```
env-wizard
  Enter accept default   ? AI hint   (empty) leave empty   q quit

  # Secret used to sign session cookies (32+ chars)
? SECRET_KEY › ?

💡 Hint:
This signs your session cookies. Generate a value with:
    openssl rand -hex 32
and paste the 64-character hex string here.

? SECRET_KEY › 9f2c…            ← you paste the real value
✔ SECRET_KEY · 9f2c…
✓ Wrote .env
```

## Why you'll like it

- 🧭 **Guided, not guesswork** — every variable's `.env.example` comment is shown
  inline as a hint, *before* you ever call the AI.
- 🤖 **Your AI, your rules** — hints come from whatever provider you pick: Claude,
  OpenAI, a local Ollama model, or any OpenAI-compatible endpoint. Nothing is
  hardcoded and no keys are stored by env-wizard.
- 🔒 **Local-friendly** — point it at `http://localhost:11434` and your secrets
  never leave your machine.
- 🧠 **Repo-aware hints** — the AI is fed your README, common config files, and
  every place the variable appears in the code, so its advice is specific.
- 💾 **Safe writes** — confirms before overwriting an existing `.env` and keeps a
  `.env.bak`.
- 🪶 **One small binary** — written in Rust, no runtime, starts instantly.

## Install

```sh
cargo install --path .
```

## Usage

Run it at the root of a repo that has a `.env.example`:

```sh
env-wizard
```

At each prompt:

| Input          | Effect                                   |
| -------------- | ---------------------------------------- |
| `Enter`        | Accept the shown default                 |
| `?` / `/hint`  | Ask the AI for a hint, then re-prompt    |
| *(empty)*      | Leave the variable empty                 |
| `q`            | Quit without writing                     |

The AI hint gathers context automatically — the README, common config files
(`docker-compose*`, `Makefile`, `settings*`, `config*`, …) and every occurrence
of the variable across the repo — then asks what value to set and how to obtain
it.

At the end, `.env` is written. If one already exists you're asked to confirm, and
the previous file is saved as `.env.bak`.

### Options

| Flag                 | Description                                          |
| -------------------- | ---------------------------------------------------- |
| `-i, --input <PATH>` | Example file to read (default `.env.example`)        |
| `-o, --output <PATH>`| Env file to write (default `.env`)                   |
| `-y, --yes`          | Accept all defaults and overwrite without confirming |
| `--no-ai`            | Disable the AI hint feature (no calls to a provider) |

## AI provider

The **first time** you request a hint (`?`), env-wizard asks which AI to use and
remembers your choice. Two kinds are supported:

- **CLI command** — pipes the prompt to a local/cloud CLI. Presets: Claude
  (`claude -p`), Ollama (`ollama run <model>`), or any custom command. These
  manage their own authentication; env-wizard stores no keys.
- **OpenAI-compatible HTTP API** — `base_url` + `model` + an env var holding the
  API key. Presets: OpenAI, local Ollama (`http://localhost:11434/v1`), or any
  compatible endpoint (LM Studio, OpenRouter, Groq, …). For local endpoints the
  key can be left empty.

### Changing the provider

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

## How it works

```
.env.example ──▶ parser ──▶ prompt loop ──▶ .env
                               │
                    type "?" ──┤
                               ▼
                   gather repo context (README, configs, grep)
                               ▼
                     your AI provider (CLI or HTTP)
                               ▼
                          hint shown inline
```

## Requirements

- A Rust toolchain to build.
- For the `?` hint: whichever provider you pick must be reachable — the chosen
  CLI on your PATH, or the HTTP endpoint up with its API key set. If it isn't,
  everything else still works and the hint reports a clear error.

## License

MIT
