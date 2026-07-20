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
    /// All variables were processed: (key, value) pairs.
    Completed(Vec<(String, String)>),
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
    let mut answers: Vec<(String, String)> = Vec::new();

    // The legend only matters for the interactive flow.
    if !opts.accept_defaults {
        print_legend(opts.no_ai);
    }

    for var in vars {
        // Non-interactive mode: take the default value as-is.
        if opts.accept_defaults {
            let value = var.default.clone().unwrap_or_default();
            answers.push((var.key.clone(), value));
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

            match trimmed {
                "q" | ":q" => return Ok(Outcome::Quit),
                "?" | "/hint" if !opts.no_ai => {
                    match hint::get_hint(repo_root, &var.key, &var.description) {
                        Ok(h) => {
                            println!("\n{}", style("💡 Hint").cyan().bold());
                            println!("{}\n", render::markdown_to_terminal(&h));
                        }
                        Err(e) => {
                            eprintln!("{} {e:#}", style("⚠  No hint:").yellow());
                        }
                    }
                    // Re-ask the same variable.
                    continue;
                }
                _ => break raw,
            }
        };

        answers.push((var.key.clone(), value));
    }

    Ok(Outcome::Completed(answers))
}

/// Prints the banner and a legend of controls, one key per line.
fn print_legend(no_ai: bool) {
    println!("{}", style("env-wizard").bold().green());
    println!("{}", style("At each prompt, type:").dim());
    print_control("Enter", "accept the suggested default");
    if !no_ai {
        print_control("?", "ask the AI for a hint");
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
