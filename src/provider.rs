//! AI provider abstraction for the hint feature.
//!
//! A provider is either a **command** (a local/cloud CLI such as `claude`,
//! `ollama`, `gemini`, `llm`, …) or an **OpenAI-compatible HTTP endpoint**
//! (OpenAI, Ollama, LM Studio, OpenRouter, …). The chosen provider is stored in
//! a small TOML config file and can be changed via `env-wizard config` or by
//! editing that file by hand.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use serde::{Deserialize, Serialize};

/// The persisted provider configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// `"command"` or `"openai"`.
    pub kind: String,
    /// Friendly name shown in the spinner (e.g. `"Claude (CLI)"`).
    #[serde(default)]
    pub label: String,
    /// Present when `kind == "command"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandProvider>,
    /// Present when `kind == "openai"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai: Option<OpenaiProvider>,
}

/// A CLI-command provider: the prompt is passed as an argument or piped to stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandProvider {
    /// Program to run (e.g. `claude`).
    pub program: String,
    /// Fixed arguments preceding the prompt (e.g. `["-p"]`).
    #[serde(default)]
    pub args: Vec<String>,
    /// `"arg"` (append the prompt) or `"stdin"` (pipe the prompt).
    pub prompt_via: String,
}

/// An OpenAI-compatible HTTP provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenaiProvider {
    /// Base URL, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    /// Model name, e.g. `gpt-4o-mini`.
    pub model: String,
    /// Env var holding the API key; empty means no auth (e.g. local Ollama).
    #[serde(default)]
    pub api_key_env: String,
}

/// Resolves the config file path (does not create anything).
///
/// `$ENV_WIZARD_CONFIG` → `$XDG_CONFIG_HOME/env-wizard/config.toml` →
/// `$HOME/.config/env-wizard/config.toml`.
pub fn config_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("ENV_WIZARD_CONFIG") {
        if !explicit.is_empty() {
            return PathBuf::from(explicit);
        }
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::config_dir)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("env-wizard").join("config.toml")
}

/// Loads the config, or `None` if the file does not exist.
pub fn load() -> Result<Option<Config>> {
    let path = config_path();
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    let cfg: Config =
        toml::from_str(&text).with_context(|| format!("invalid config in {}", path.display()))?;
    Ok(Some(cfg))
}

/// Saves the config as TOML, creating the parent directory. Returns the path.
pub fn save(cfg: &Config) -> Result<PathBuf> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(cfg).context("failed to serialize config")?;
    std::fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// Returns the stored config, running the interactive picker on first use.
pub fn ensure_configured() -> Result<Config> {
    match load()? {
        Some(cfg) => Ok(cfg),
        None => {
            println!(
                "\n{}",
                style("No AI provider configured yet — let's pick one.").bold()
            );
            configure_interactive()
        }
    }
}

/// Interactive provider picker. Saves the result and prints how to change it.
pub fn configure_interactive() -> Result<Config> {
    let theme = ColorfulTheme::default();
    let choices = [
        "Claude (CLI)",
        "Ollama — local (CLI)",
        "OpenAI (API)",
        "Ollama — local (API)",
        "LM Studio — local (API)",
        "OpenRouter (API)",
        "Groq (API)",
        "Other CLI command…",
        "Other OpenAI-compatible API…",
    ];
    let selection = Select::with_theme(&theme)
        .with_prompt("AI provider for hints")
        .items(&choices)
        .default(0)
        .interact()?;

    let cfg = match selection {
        0 => Config {
            kind: "command".into(),
            label: "Claude (CLI)".into(),
            command: Some(CommandProvider {
                program: "claude".into(),
                args: vec!["-p".into()],
                prompt_via: "arg".into(),
            }),
            openai: None,
        },
        1 => {
            let model = ask_default(&theme, "Ollama model", "llama3")?;
            Config {
                kind: "command".into(),
                label: format!("Ollama/{model} (CLI)"),
                command: Some(CommandProvider {
                    program: "ollama".into(),
                    args: vec!["run".into(), model],
                    prompt_via: "stdin".into(),
                }),
                openai: None,
            }
        }
        2 => {
            let model = ask_default(&theme, "OpenAI model", "gpt-4o-mini")?;
            Config {
                kind: "openai".into(),
                label: format!("OpenAI/{model}"),
                command: None,
                openai: Some(OpenaiProvider {
                    base_url: "https://api.openai.com/v1".into(),
                    model,
                    api_key_env: "OPENAI_API_KEY".into(),
                }),
            }
        }
        3 => {
            let model = ask_default(&theme, "Ollama model", "llama3")?;
            Config {
                kind: "openai".into(),
                label: format!("Ollama/{model} (API)"),
                command: None,
                openai: Some(OpenaiProvider {
                    base_url: "http://localhost:11434/v1".into(),
                    model,
                    api_key_env: String::new(),
                }),
            }
        }
        4 => {
            let model = ask_default(&theme, "LM Studio model", "local-model")?;
            Config {
                kind: "openai".into(),
                label: format!("LM Studio/{model} (API)"),
                command: None,
                openai: Some(OpenaiProvider {
                    base_url: "http://localhost:1234/v1".into(),
                    model,
                    api_key_env: String::new(),
                }),
            }
        }
        5 => {
            let model = ask_default(&theme, "OpenRouter model", "anthropic/claude-3.5-sonnet")?;
            Config {
                kind: "openai".into(),
                label: format!("OpenRouter/{model} (API)"),
                command: None,
                openai: Some(OpenaiProvider {
                    base_url: "https://openrouter.ai/api/v1".into(),
                    model,
                    api_key_env: "OPENROUTER_API_KEY".into(),
                }),
            }
        }
        6 => {
            let model = ask_default(&theme, "Groq model", "llama-3.3-70b-versatile")?;
            Config {
                kind: "openai".into(),
                label: format!("Groq/{model} (API)"),
                command: None,
                openai: Some(OpenaiProvider {
                    base_url: "https://api.groq.com/openai/v1".into(),
                    model,
                    api_key_env: "GROQ_API_KEY".into(),
                }),
            }
        }
        7 => {
            let program: String = Input::with_theme(&theme)
                .with_prompt("Program (e.g. gemini, llm)")
                .interact_text()?;
            let args_raw: String = Input::with_theme(&theme)
                .with_prompt("Fixed arguments before the prompt (space-separated)")
                .allow_empty(true)
                .default("-p".into())
                .interact_text()?;
            let via_idx = Select::with_theme(&theme)
                .with_prompt("How is the prompt passed?")
                .items(&["as the last argument", "piped to stdin"])
                .default(0)
                .interact()?;
            let args: Vec<String> = args_raw.split_whitespace().map(|s| s.to_string()).collect();
            Config {
                kind: "command".into(),
                label: format!("{program} (CLI)"),
                command: Some(CommandProvider {
                    program,
                    args,
                    prompt_via: if via_idx == 0 {
                        "arg".into()
                    } else {
                        "stdin".into()
                    },
                }),
                openai: None,
            }
        }
        _ => {
            let base_url: String = Input::with_theme(&theme)
                .with_prompt("Base URL")
                .default("http://localhost:11434/v1".into())
                .interact_text()?;
            let model: String = Input::with_theme(&theme)
                .with_prompt("Model")
                .interact_text()?;
            let api_key_env: String = Input::with_theme(&theme)
                .with_prompt("Env var holding the API key (empty for none)")
                .allow_empty(true)
                .interact_text()?;
            Config {
                kind: "openai".into(),
                label: format!("{model} (API)"),
                command: None,
                openai: Some(OpenaiProvider {
                    base_url,
                    model,
                    api_key_env,
                }),
            }
        }
    };

    let path = save(&cfg)?;
    println!(
        "{} {} → {}",
        style("✓ Saved provider").green().bold(),
        style(&cfg.label).bold(),
        path.display()
    );
    println!(
        "{}",
        style(format!(
            "  Change it anytime with `env-wizard config` or by editing {}",
            path.display()
        ))
        .dim()
    );
    Ok(cfg)
}

/// Asks for a value with a default.
fn ask_default(theme: &ColorfulTheme, prompt: &str, default: &str) -> Result<String> {
    Ok(Input::with_theme(theme)
        .with_prompt(prompt)
        .default(default.to_string())
        .interact_text()?)
}

/// Sends `prompt` to the configured provider and returns the hint text.
pub fn run(cfg: &Config, prompt: &str) -> Result<String> {
    match cfg.kind.as_str() {
        "command" => {
            let c = cfg
                .command
                .as_ref()
                .context("config kind is `command` but no [command] section is present")?;
            run_command(c, prompt)
        }
        "openai" => {
            let o = cfg
                .openai
                .as_ref()
                .context("config kind is `openai` but no [openai] section is present")?;
            run_http(o, prompt)
        }
        other => anyhow::bail!("unknown provider kind `{other}` in config"),
    }
}

/// Runs a CLI-command provider.
fn run_command(c: &CommandProvider, prompt: &str) -> Result<String> {
    let mut cmd = Command::new(&c.program);
    cmd.args(&c.args);

    let output = if c.prompt_via == "stdin" {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().with_context(|| {
            format!(
                "failed to launch `{}` — make sure it is installed and on your PATH",
                c.program
            )
        })?;
        let mut stdin = child
            .stdin
            .take()
            .context("failed to open stdin of the provider command")?;
        // A child that ignores stdin may close it before we finish writing; a
        // BrokenPipe here is harmless, so don't fail the hint on it.
        match stdin.write_all(prompt.as_bytes()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(e) => return Err(e).context("failed to write the prompt to the provider command"),
        }
        drop(stdin); // close stdin so the child sees EOF and can proceed
        child
            .wait_with_output()
            .context("provider command failed")?
    } else {
        cmd.arg(prompt);
        cmd.output().with_context(|| {
            format!(
                "failed to launch `{}` — make sure it is installed and on your PATH",
                c.program
            )
        })?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("`{}` failed: {}", c.program, stderr.trim());
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        anyhow::bail!("`{}` returned no content", c.program);
    }
    Ok(text)
}

/// Runs an OpenAI-compatible HTTP provider.
fn run_http(o: &OpenaiProvider, prompt: &str) -> Result<String> {
    let url = format!("{}/chat/completions", o.base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": o.model,
        "messages": [{ "role": "user", "content": prompt }],
    });

    // Bounded timeouts so a wrong/dead endpoint fails fast instead of hanging.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build();

    let mut req = agent.post(&url).set("Content-Type", "application/json");
    if !o.api_key_env.is_empty() {
        let key = std::env::var(&o.api_key_env).with_context(|| {
            format!(
                "API key not found — set the `{}` environment variable",
                o.api_key_env
            )
        })?;
        req = req.set("Authorization", &format!("Bearer {key}"));
    }

    let resp = match req.send_json(body) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, response)) => {
            // The provider is reachable and answered — the failure is in the
            // response itself (bad/missing API key, rate limit, bad model
            // name, …). Surface the status and body instead of the generic
            // "unreachable or timed out" message, which would be misleading.
            let body = response.into_string().unwrap_or_default();
            let snippet: String = body.chars().take(300).collect();
            anyhow::bail!("provider returned HTTP {code} for {url}: {snippet}");
        }
        Err(ureq::Error::Transport(e)) => {
            return Err(anyhow::Error::new(e)).with_context(|| {
                format!("request to {url} failed (endpoint unreachable or timed out?)")
            });
        }
    };
    let json: serde_json::Value = resp.into_json().context("invalid JSON from provider")?;

    let content = json["choices"][0]["message"]["content"]
        .as_str()
        .context("unexpected response shape (no choices[0].message.content)")?
        .trim()
        .to_string();
    if content.is_empty() {
        anyhow::bail!("provider returned an empty message");
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_rejects_unknown_kind() {
        let cfg = Config {
            kind: "nope".into(),
            label: String::new(),
            command: None,
            openai: None,
        };
        let err = run(&cfg, "hi").unwrap_err().to_string();
        assert!(err.contains("unknown provider kind"), "{err}");
    }

    #[test]
    fn run_requires_matching_section() {
        let cfg = Config {
            kind: "command".into(),
            label: String::new(),
            command: None,
            openai: None,
        };
        let err = run(&cfg, "hi").unwrap_err().to_string();
        assert!(err.contains("no [command] section"), "{err}");
    }

    #[test]
    fn config_path_honours_explicit_env() {
        std::env::set_var("ENV_WIZARD_CONFIG", "/tmp/ew-explicit.toml");
        assert_eq!(config_path(), PathBuf::from("/tmp/ew-explicit.toml"));
        std::env::remove_var("ENV_WIZARD_CONFIG");
    }

    // --- run_command --------------------------------------------------------
    // Exercised with real, near-universal coreutils rather than mocks, since
    // it's just spawning a process — no network involved. Unix-only, matching
    // the existing platform-conditional test pattern in src/writer.rs.

    #[cfg(unix)]
    fn command(program: &str, args: &[&str], prompt_via: &str) -> CommandProvider {
        CommandProvider {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            prompt_via: prompt_via.into(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_command_arg_mode_passes_prompt_as_last_argument() {
        let out = run_command(&command("echo", &[], "arg"), "hello arg mode").unwrap();
        assert_eq!(out, "hello arg mode");
    }

    #[cfg(unix)]
    #[test]
    fn run_command_stdin_mode_pipes_prompt() {
        let out = run_command(&command("cat", &[], "stdin"), "hello stdin mode").unwrap();
        assert_eq!(out, "hello stdin mode");
    }

    #[cfg(unix)]
    #[test]
    fn run_command_surfaces_nonzero_exit() {
        let err = run_command(&command("false", &[], "arg"), "hi")
            .unwrap_err()
            .to_string();
        assert!(err.contains("`false` failed"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn run_command_rejects_empty_output() {
        let err = run_command(&command("true", &[], "arg"), "hi")
            .unwrap_err()
            .to_string();
        assert!(err.contains("returned no content"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn run_command_reports_launch_failure() {
        let err = run_command(
            &command("definitely-not-a-real-binary-xyz", &[], "arg"),
            "hi",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("failed to launch"), "{err}");
    }

    // --- run_http ------------------------------------------------------------
    // A tiny hand-rolled TCP mock: real HTTP over loopback, no dependency on a
    // mocking crate, and no network access outside the machine.

    fn spawn_http_mock(response: &'static str) -> String {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));

                // Read the full request — headers, then exactly as much body as
                // its own Content-Length declares — before responding. Draining
                // it fully avoids leaving unread bytes in the socket buffer,
                // which can turn a plain close into a TCP reset raced against
                // our response under load (the source of this mock's original
                // flakiness).
                let mut received = Vec::new();
                let mut buf = [0u8; 4096];
                let header_end = loop {
                    if let Some(pos) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(pos + 4);
                    }
                    if received.len() > 64 * 1024 {
                        break None;
                    }
                    match stream.read(&mut buf) {
                        Ok(0) => break None,
                        Ok(n) => received.extend_from_slice(&buf[..n]),
                        Err(_) => break None,
                    }
                };

                if let Some(header_end) = header_end {
                    let header_text = String::from_utf8_lossy(&received[..header_end]);
                    let content_length: usize = header_text
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("Content-Length:")
                                .or(l.strip_prefix("content-length:"))
                        })
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    let body_so_far = received.len() - header_end;
                    let mut remaining = content_length.saturating_sub(body_so_far);
                    while remaining > 0 {
                        let n = std::cmp::min(remaining, buf.len());
                        match stream.read(&mut buf[..n]) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => remaining -= read,
                        }
                    }
                }

                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });
        format!("http://{addr}")
    }

    fn openai(base_url: String) -> OpenaiProvider {
        OpenaiProvider {
            base_url,
            model: "test-model".into(),
            api_key_env: String::new(),
        }
    }

    #[test]
    fn run_http_returns_message_content_on_success() {
        let body = r#"{"choices":[{"message":{"content":"hello from mock"}}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let url = spawn_http_mock(Box::leak(response.into_boxed_str()));
        let out = run_http(&openai(url), "hi").unwrap();
        assert_eq!(out, "hello from mock");
    }

    #[test]
    fn run_http_surfaces_status_code_and_body_on_error() {
        let body = r#"{"error":"invalid api key"}"#;
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let url = spawn_http_mock(Box::leak(response.into_boxed_str()));
        let err = run_http(&openai(url), "hi").unwrap_err().to_string();
        assert!(err.contains("401"), "{err}");
        assert!(err.contains("invalid api key"), "{err}");
        // Must NOT be misreported as a connectivity issue — that's the bug fixed.
        assert!(!err.contains("unreachable or timed out"), "{err}");
    }
}
