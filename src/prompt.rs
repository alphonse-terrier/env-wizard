//! Interactive input loop for the variables, with handling of the `?` command
//! that triggers an AI hint.

use std::path::Path;

use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Input};

use crate::hint;
use crate::parser::EnvVar;
use crate::render;

/// Input result: on `q`, we stop and return `Quit`.
pub enum Outcome {
    /// All variables were processed: (variable, value) pairs, in order.
    Completed(Vec<(EnvVar, String)>),
    /// The user asked to quit.
    Quit,
}

/// Options controlling the input loop.
pub struct Options {
    /// Disable the AI hint (`--no-ai`).
    pub no_ai: bool,
    /// Accept all default values without prompting (`--yes`).
    pub accept_defaults: bool,
}

/// Interactively asks for each variable.
pub fn run(repo_root: &Path, vars: &[EnvVar], opts: &Options) -> Result<Outcome> {
    let theme = ColorfulTheme::default();
    let mut answers: Vec<(EnvVar, String)> = Vec::new();

    // The legend only matters for the interactive flow.
    if !opts.accept_defaults {
        print_legend(opts.no_ai);
    }

    for var in vars {
        // Non-interactive mode: take the default value as-is.
        if opts.accept_defaults {
            let value = var.default.clone().unwrap_or_default();
            answers.push((var.clone(), value));
            continue;
        }

        // Show the .env.example comment as a hint, before any AI is involved.
        println!();
        for line in var.description.lines() {
            println!("{}", style(format!("  # {line}")).dim());
        }

        let value = loop {
            let mut input = Input::<String>::with_theme(&theme);
            input = input.with_prompt(&var.key).allow_empty(true);
            if let Some(def) = &var.default {
                input = input.default(def.clone());
            }

            let raw = input.interact_text()?;
            let trimmed = raw.trim();

            if trimmed == "q" || trimmed == ":q" {
                return Ok(Outcome::Quit);
            }

            // `?` / `/hint` / `/ask`, optionally followed by a free-text question.
            if !opts.no_ai {
                if let Some(question) = parse_ask_command(trimmed) {
                    match hint::get_hint(repo_root, &var.key, &var.description, question.as_deref())
                    {
                        Ok(h) => {
                            let title = if question.is_some() {
                                "💬 Answer"
                            } else {
                                "💡 Hint"
                            };
                            println!("\n{}", style(title).cyan().bold());
                            println!("{}\n", render::markdown_to_terminal(&h));
                        }
                        Err(e) => {
                            eprintln!("{} {e:#}", style("⚠  No hint:").yellow());
                        }
                    }
                    // Re-ask the same variable.
                    continue;
                }
            }

            break raw;
        };

        answers.push((var.clone(), value));
    }

    Ok(Outcome::Completed(answers))
}

/// Parses an AI-hint command.
///
/// Returns `None` when the input is a normal value. Returns `Some(None)` for a
/// bare hint request (`?`, `/hint`, `/ask`), and `Some(Some(question))` when a
/// free-text question follows (`? what format?`, `/ask where do I get this?`).
fn parse_ask_command(input: &str) -> Option<Option<String>> {
    let t = input.trim();

    // `?` with the question attached directly or after a space.
    if let Some(rest) = t.strip_prefix('?') {
        let q = rest.trim();
        return Some((!q.is_empty()).then(|| q.to_string()));
    }

    // `/hint` / `/ask`, alone or followed by whitespace + a question.
    for cmd in ["/hint", "/ask"] {
        if t == cmd {
            return Some(None);
        }
        if let Some(rest) = t.strip_prefix(cmd) {
            if rest.starts_with(char::is_whitespace) {
                let q = rest.trim();
                return Some((!q.is_empty()).then(|| q.to_string()));
            }
        }
    }

    None
}

/// Prints the banner and a legend of controls, one key per line.
fn print_legend(no_ai: bool) {
    println!("{}", style("env-wizard").bold().green());
    println!("{}", style("At each prompt, type:").dim());
    print_control("Enter", "accept the suggested default");
    if !no_ai {
        print_control("?", "ask the AI for a hint");
        print_control("? …", "ask the AI a specific question about this variable");
    }
    print_control("(nothing)", "leave this variable empty");
    print_control("q", "quit without saving");
    if !no_ai {
        println!(
            "{}",
            style("Change the AI provider anytime with `env-wizard config`.").dim()
        );
    }
    println!();
}

/// Prints one control row: a highlighted "keycap" and its description.
fn print_control(key: &str, description: &str) {
    // Fixed-width keycap so every row lines up, rendered as reversed video.
    let cap = format!(" {key:^9} ");
    println!("  {}  {}", style(cap).reverse(), style(description).dim());
}

#[cfg(test)]
mod tests {
    use super::parse_ask_command;

    #[test]
    fn bare_hint_commands() {
        assert_eq!(parse_ask_command("?"), Some(None));
        assert_eq!(parse_ask_command("/hint"), Some(None));
        assert_eq!(parse_ask_command("/ask"), Some(None));
    }

    #[test]
    fn hint_with_question() {
        assert_eq!(
            parse_ask_command("? what format is expected?"),
            Some(Some("what format is expected?".to_string()))
        );
        assert_eq!(
            parse_ask_command("?where do I get it"),
            Some(Some("where do I get it".to_string()))
        );
        assert_eq!(
            parse_ask_command("/ask which service issues this"),
            Some(Some("which service issues this".to_string()))
        );
    }

    #[test]
    fn normal_values_are_not_commands() {
        assert_eq!(parse_ask_command("postgres://localhost/db"), None);
        assert_eq!(parse_ask_command("8080"), None);
        // `/askew` is not the `/ask` command (no whitespace boundary).
        assert_eq!(parse_ask_command("/askew"), None);
    }
}
