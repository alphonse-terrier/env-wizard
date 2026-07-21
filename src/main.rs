//! env-wizard — interactive assistant to fill a `.env` from a `.env.example`,
//! with on-demand AI hints from a configurable AI provider (cloud or local).
//!
//! It also understands structured config *templates*
//! (`config.example.toml`/`.yaml`/`.json`, or a `.sample`/`.dist`/`.template`
//! variant): the same walkthrough, but reading/writing a typed, nested file
//! instead of a flat `.env`. See [`env_wizard::config`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm};

use env_wizard::config::{self, ConfigDoc, Field};
use env_wizard::parser::{self, EnvVar};
use env_wizard::prompt::{self, Options, Outcome};
use env_wizard::provider;
use env_wizard::repo;
use env_wizard::scan;
use env_wizard::writer::{self, WriteOutcome};

/// Common names for the example file, tried in order when `--input` is omitted.
const EXAMPLE_ALIASES: &[&str] = &[
    ".env.example",
    ".env.sample",
    ".env.dist",
    ".env.template",
    "env.example",
];

/// Interactive `.env` filler driven by your project's `.env.example` (or a
/// structured config template).
#[derive(Parser, Debug)]
#[command(name = "env-wizard", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to the example file to read. If omitted, tries common names in
    /// order: .env.example, .env.sample, .env.dist, .env.template,
    /// env.example — then a structured config template (a `.toml`/`.yaml`/
    /// `.json` file whose name contains `example`/`sample`/`dist`/`template`).
    #[arg(short, long)]
    input: Option<PathBuf>,

    /// Path to the file to write. Defaults to `.env` for a dotenv example, or
    /// the template's name with the marker segment stripped for a structured
    /// config template (e.g. `config.example.toml` -> `config.toml`).
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Accept all default values and overwrite without confirmation.
    #[arg(short = 'y', long)]
    yes: bool,

    /// Disable the AI hint feature (no calls to the AI provider).
    #[arg(long)]
    no_ai: bool,

    /// Also include environment variables discovered in the code, not just the
    /// ones declared in the example file. Dotenv only — has no effect for a
    /// structured config template.
    #[arg(long)]
    from_code: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Choose or change the AI provider used for hints.
    Config,
    /// Audit: compare variables used in code against the `.env.example`.
    Scan {
        /// Exit with status 1 if drift is found (missing or unused
        /// variables). Useful as a CI check; the default (no flag) always
        /// exits 0, matching the plain "read-only audit" behavior.
        #[arg(long)]
        check: bool,
    },
    /// Generate a shell completion script (bash, zsh, fish, elvish, powershell).
    Completions {
        /// Shell to generate completions for.
        shell: clap_complete::Shell,
    },
}

/// Which shape the resolved input file has, and how to read/write it.
enum InputFormat {
    /// A flat `KEY=value` dotenv-style file.
    Dotenv,
    /// A structured config template in the given format.
    Config(config::Format),
}

/// The example/template file to read, and how to interpret it.
struct ResolvedInput {
    path: PathBuf,
    format: InputFormat,
}

/// The parsed input, ready to prompt for and then write back.
enum Source {
    Dotenv(Vec<EnvVar>),
    Config(Box<dyn ConfigDoc>, Vec<Field>),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let resolved = resolve_input(cli.input.as_deref());

    match cli.command {
        Some(Commands::Config) => {
            provider::configure_interactive()?;
            return Ok(());
        }
        Some(Commands::Scan { check }) => return run_scan(&resolved, check),
        Some(Commands::Completions { shell }) => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "env-wizard",
                &mut std::io::stdout(),
            );
            return Ok(());
        }
        None => {}
    }

    let repo_root = repo_root_of(&resolved.path);
    let source = collect_vars(&cli, &resolved, &repo_root)?;
    let prompt_vars = prompt_vars_for(&source);

    if prompt_vars.is_empty() {
        println!(
            "{}",
            style(format!(
                "No variables found in {}.",
                resolved.path.display()
            ))
            .yellow()
        );
        return Ok(());
    }

    let opts = Options {
        no_ai: cli.no_ai,
        accept_defaults: cli.yes,
    };

    let answers = match prompt::run(&repo_root, &prompt_vars, &opts)? {
        Outcome::Completed(a) => a,
        Outcome::Quit => {
            println!("{}", style("Aborted — no file written.").yellow());
            return Ok(());
        }
    };

    let output_path = cli
        .output
        .clone()
        .unwrap_or_else(|| default_output_for(&resolved));

    let outcome = match source {
        Source::Dotenv(_) => {
            let outcome = writer::write_env(&output_path, &answers, cli.yes)?;
            if matches!(outcome, WriteOutcome::Written) && !cli.from_code && !cli.yes {
                offer_missing_code_vars(&repo_root, &output_path, &answers, &opts)?;
            }
            outcome
        }
        Source::Config(mut doc, fields) => {
            let values: Vec<String> = answers.into_iter().map(|(_, v)| v).collect();
            config::apply_answers(doc.as_mut(), &fields, &values)?;
            writer::write_file(
                &output_path,
                &doc.render(),
                cli.yes,
                /* restrict = */ false,
            )?
        }
    };

    match outcome {
        WriteOutcome::Written => {
            println!(
                "{} {}",
                style("✓ Wrote").green().bold(),
                output_path.display()
            );
        }
        WriteOutcome::Aborted => {
            println!(
                "{}",
                style("Overwrite cancelled — no file written.").yellow()
            );
        }
    }

    Ok(())
}

/// Builds the `EnvVar` list to hand to the shared interactive prompt.
///
/// For a config source, each scalar leaf becomes one `EnvVar` (dotted path as
/// `key`, the example's value as `default`, its comment as `description`) —
/// this is what lets `prompt::run`, AI hints, `--yes`, and quit all work
/// unchanged for structured formats. The answers are later applied back onto
/// the `ConfigDoc` *positionally*, never by re-parsing the dotted `key`, so
/// keys that themselves contain a literal dot are never ambiguous.
fn prompt_vars_for(source: &Source) -> Vec<EnvVar> {
    match source {
        Source::Dotenv(vars) => vars.clone(),
        Source::Config(_, fields) => fields
            .iter()
            .map(|f| EnvVar {
                key: f.display.clone(),
                default: Some(f.original.clone()),
                description: f.description.clone(),
            })
            .collect(),
    }
}

/// The output path to use when `-o`/`--output` wasn't given.
fn default_output_for(resolved: &ResolvedInput) -> PathBuf {
    match resolved.format {
        InputFormat::Dotenv => PathBuf::from(".env"),
        InputFormat::Config(format) => config::derive_output_name(&resolved.path, format),
    }
}

/// True if `path`'s filename is one of the recognized dotenv aliases, or
/// starts with `.env` (e.g. `.env`, `.env.local`). Structured-format content
/// sniffing never runs on these — a dotenv-shaped file is always treated as
/// dotenv, even if quoted `KEY="value"` lines happen to also parse as valid
/// TOML, so existing dotenv-based projects keep behaving exactly as before.
fn is_dotenv_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.starts_with(".env") || EXAMPLE_ALIASES.contains(&name)
}

/// Resolves which example file to read, relative to the current directory.
fn resolve_input(explicit: Option<&Path>) -> ResolvedInput {
    resolve_input_from(Path::new("."), explicit)
}

/// Same as [`resolve_input`], but checks candidates under `base` instead of
/// the current directory — lets tests probe a temp dir without mutating the
/// process-global current directory.
///
/// Precedence: an explicit `--input` always wins (format detected from its
/// content, see [`detect_explicit_format`]). Otherwise, the dotenv aliases
/// (see [`EXAMPLE_ALIASES`]) are tried first, to keep existing dotenv-based
/// projects behaving exactly as before. Only if none of those exist do we
/// look for a structured config template (a file whose name carries a
/// template marker and whose content is a recognized structured format). If
/// more than one such template is found, we can't guess which one the user
/// means, so we fall back to the dotenv default and ask them to pass
/// `--input` explicitly.
fn resolve_input_from(base: &Path, explicit: Option<&Path>) -> ResolvedInput {
    if let Some(path) = explicit {
        let format = detect_explicit_format(base, path);
        return ResolvedInput {
            path: path.to_path_buf(),
            format,
        };
    }

    let dotenv_candidate = resolve_input_path_from(base, None);
    if base.join(&dotenv_candidate).exists() {
        return ResolvedInput {
            path: dotenv_candidate,
            format: InputFormat::Dotenv,
        };
    }

    match find_config_template(base) {
        FoundTemplates::One(name, format) => {
            println!(
                "{}",
                style(format!("Using {name} (no {} found).", EXAMPLE_ALIASES[0])).dim()
            );
            ResolvedInput {
                path: PathBuf::from(name),
                format: InputFormat::Config(format),
            }
        }
        FoundTemplates::Many(names) => {
            println!(
                "{}",
                style(format!(
                    "Multiple config templates found ({}) — pass --input to pick one.",
                    names.join(", ")
                ))
                .yellow()
            );
            ResolvedInput {
                path: dotenv_candidate,
                format: InputFormat::Dotenv,
            }
        }
        FoundTemplates::None => ResolvedInput {
            path: dotenv_candidate,
            format: InputFormat::Dotenv,
        },
    }
}

/// Reads `path` to a string, refusing files larger than [`repo::MAX_FILE_BYTES`]
/// — the same cap `repo::text_files` applies during code scanning — so format
/// auto-detection and example/template parsing can't be made to slurp an
/// unexpectedly huge file into memory.
fn read_capped(path: &Path) -> std::io::Result<String> {
    let len = std::fs::metadata(path)?.len();
    if len > repo::MAX_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} is {len} bytes, over the {}-byte limit env-wizard reads for example/template files",
                path.display(),
                repo::MAX_FILE_BYTES
            ),
        ));
    }
    std::fs::read_to_string(path)
}

/// Detects the [`InputFormat`] of an explicit `--input` path.
///
/// A dotenv-shaped filename (see [`is_dotenv_name`]) is always [`InputFormat::Dotenv`]
/// without reading it — content sniffing never crosses that boundary. Otherwise
/// the file is read and [`config::detect_format_from_content`] decides, falling
/// back to the extension when the content isn't decisive; either way a
/// recognized format wins even without a template marker, since an explicit
/// `--input` is an unambiguous statement of intent. If the file can't be read
/// (e.g. it doesn't exist yet), detection falls back to the extension alone so
/// the existing "could not read" error path is unaffected.
fn detect_explicit_format(base: &Path, path: &Path) -> InputFormat {
    if is_dotenv_name(path) {
        return InputFormat::Dotenv;
    }

    let extension_format = || {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(config::format_from_extension)
    };

    match read_capped(&base.join(path)) {
        Ok(content) => config::detect_format_from_content(&content)
            .or_else(extension_format)
            .map(InputFormat::Config)
            .unwrap_or(InputFormat::Dotenv),
        Err(_) => extension_format()
            .map(InputFormat::Config)
            .unwrap_or(InputFormat::Dotenv),
    }
}

/// Resolves which dotenv-style example file to read, relative to `base`.
///
/// An explicit `--input` is honored verbatim — no substitution. Otherwise, the
/// first [`EXAMPLE_ALIASES`] entry that exists on disk is used; if none exist,
/// returns the conventional `.env.example` so downstream "no example" messaging
/// (and the code-scan fallback) stays unchanged.
fn resolve_input_path_from(base: &Path, explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    for name in EXAMPLE_ALIASES {
        if base.join(name).exists() {
            if *name != EXAMPLE_ALIASES[0] {
                println!(
                    "{}",
                    style(format!("Using {name} (no {} found).", EXAMPLE_ALIASES[0])).dim()
                );
            }
            return PathBuf::from(name);
        }
    }
    PathBuf::from(EXAMPLE_ALIASES[0])
}

enum FoundTemplates {
    None,
    One(String, config::Format),
    Many(Vec<String>),
}

/// Scans `base` (non-recursively) for structured config templates: a file
/// whose name carries a template marker (see [`config::has_template_marker`])
/// and whose content — read and sniffed via
/// [`config::detect_format_from_content`], falling back to its extension — is
/// a recognized structured format. Dotenv-shaped filenames are skipped
/// outright (see [`is_dotenv_name`]); their content is never sniffed here.
fn find_config_template(base: &Path) -> FoundTemplates {
    let mut matches = Vec::new();
    let Ok(entries) = std::fs::read_dir(base) else {
        return FoundTemplates::None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || is_dotenv_name(&path) || !config::has_template_marker(&path) {
            continue;
        }
        let Ok(content) = read_capped(&path) else {
            continue;
        };
        if let Some(format) = config::resolve_template_format(&path, &content) {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                matches.push((name.to_string(), format));
            }
        }
    }
    matches.sort_by(|a, b| a.0.cmp(&b.0));

    match matches.len() {
        0 => FoundTemplates::None,
        1 => {
            let (name, format) = matches.into_iter().next().expect("length checked above");
            FoundTemplates::One(name, format)
        }
        _ => FoundTemplates::Many(matches.into_iter().map(|(name, _)| name).collect()),
    }
}

/// The directory to scan / gather context from: the input file's parent, or cwd.
fn repo_root_of(input: &Path) -> PathBuf {
    input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Builds the [`Source`] to prompt from: a structured config template, or a
/// dotenv example (applying the `--from-code` augmentation and the
/// no-example fallback, both of which are dotenv-only).
fn collect_vars(cli: &Cli, resolved: &ResolvedInput, repo_root: &Path) -> Result<Source> {
    match resolved.format {
        InputFormat::Config(format) => {
            if cli.from_code {
                println!(
                    "{}",
                    style("--from-code has no effect for structured config templates; ignoring.")
                        .yellow()
                );
            }
            collect_config_vars(&resolved.path, format)
        }
        InputFormat::Dotenv => {
            collect_dotenv_vars(cli, &resolved.path, repo_root).map(Source::Dotenv)
        }
    }
}

/// Reads and parses a structured config template.
fn collect_config_vars(input: &Path, format: config::Format) -> Result<Source> {
    let content =
        read_capped(input).with_context(|| format!("could not read {}", input.display()))?;
    let doc = config::open(format, &content)?;
    let fields = doc.fields();
    Ok(Source::Config(doc, fields))
}

/// Builds the list of variables to prompt for, applying the example file, the
/// `--from-code` augmentation, and the no-example fallback.
fn collect_dotenv_vars(cli: &Cli, input: &Path, repo_root: &Path) -> Result<Vec<EnvVar>> {
    let example = match read_capped(input) {
        Ok(content) => Some(parser::parse(&content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("could not read {}", input.display())),
    };

    match (example, cli.from_code) {
        (Some(ex), false) => Ok(ex),
        (Some(ex), true) => {
            let code = scan::to_env_vars(&scan::scan_env_vars(repo_root));
            Ok(merge_vars(ex, code))
        }
        (None, _) => {
            // Fallback: no example file — derive the list from the code.
            let found = scan::scan_env_vars(repo_root);
            if found.is_empty() {
                anyhow::bail!(
                    "could not find {} and detected no environment variables in the code — \
                     run env-wizard from the repo root, or pass --input",
                    input.display()
                );
            }
            println!(
                "{}",
                style(format!(
                    "No {} found — using {} variable(s) detected in the code.",
                    input.display(),
                    found.len()
                ))
                .yellow()
            );
            Ok(scan::to_env_vars(&found))
        }
    }
}

/// Appends code-discovered vars whose key isn't already declared in `example`.
fn merge_vars(mut example: Vec<EnvVar>, code: Vec<EnvVar>) -> Vec<EnvVar> {
    let declared: HashSet<String> = example.iter().map(|v| v.key.clone()).collect();
    for v in code {
        if !declared.contains(&v.key) {
            example.push(v);
        }
    }
    example
}

/// Vars detected in code whose key isn't among `known_keys` — the keys that were
/// actually prompted for and written in this run. This is the same "used in code
/// but missing" diff `env-wizard scan` reports, but keyed off the actual
/// prompted/answered set rather than a separately re-derived "declared" set, so
/// the two can't drift apart.
fn missing_code_vars(repo_root: &Path, known_keys: &HashSet<String>) -> Vec<EnvVar> {
    let used = scan::scan_env_vars(repo_root);
    let missing: std::collections::BTreeMap<String, String> = used
        .into_iter()
        .filter(|(k, _)| !known_keys.contains(k))
        .collect();
    scan::to_env_vars(&missing)
}

/// After a successful dotenv `.env` write (interactive, non-`--from-code` run
/// only — see the guard at the call site), offers to add variables detected in
/// the code but missing from what was just written. Does nothing if there's
/// nothing missing, or if the user declines/quits.
fn offer_missing_code_vars(
    repo_root: &Path,
    output_path: &Path,
    answers: &[(EnvVar, String)],
    opts: &Options,
) -> Result<()> {
    let known_keys: HashSet<String> = answers.iter().map(|(v, _)| v.key.clone()).collect();
    let missing = missing_code_vars(repo_root, &known_keys);
    if missing.is_empty() {
        return Ok(());
    }

    println!();
    println!(
        "{}",
        style(format!(
            "Found {} variable(s) used in code but missing from {}:",
            missing.len(),
            output_path.display()
        ))
        .yellow()
        .bold()
    );
    for var in &missing {
        println!(
            "  {} {}",
            style(format!("• {}", var.key)).yellow(),
            style(&var.description).dim()
        );
    }
    println!();

    let confirmed = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Add them to the .env file now?")
        .default(false)
        .interact()?;
    if !confirmed {
        return Ok(());
    }

    let new_answers = match prompt::run(repo_root, &missing, opts)? {
        Outcome::Completed(a) => a,
        Outcome::Quit => {
            println!(
                "{}",
                style("Aborted — no additional variables added.").yellow()
            );
            return Ok(());
        }
    };

    writer::append_env(output_path, &new_answers)?;
    println!(
        "{} {} additional variable(s) to {}",
        style("✓ Appended").green().bold(),
        new_answers.len(),
        output_path.display()
    );

    Ok(())
}

/// `env-wizard scan`: read-only audit of code usage vs the example file.
///
/// With `check`, exits with status 1 if any drift is found (missing or
/// unused variables), so `scan --check` can gate CI. Without it (the
/// default), always exits 0 — it's just a report.
fn run_scan(resolved: &ResolvedInput, check: bool) -> Result<()> {
    if matches!(resolved.format, InputFormat::Config(_)) {
        println!(
            "{}",
            style("scan is only supported for .env files, not structured config templates.")
                .yellow()
        );
        return Ok(());
    }
    let input = &resolved.path;
    let repo_root = repo_root_of(input);
    let used = scan::scan_env_vars(&repo_root);

    let declared: Vec<String> = match read_capped(input) {
        Ok(content) => parser::parse(&content).into_iter().map(|v| v.key).collect(),
        Err(_) => Vec::new(),
    };
    let declared_set: HashSet<&str> = declared.iter().map(String::as_str).collect();
    let used_set: HashSet<&str> = used.keys().map(String::as_str).collect();

    // No example file: just list what was discovered in the code.
    if declared.is_empty() {
        println!(
            "{}",
            style(format!(
                "No {} — variables detected in code:",
                input.display()
            ))
            .bold()
        );
        if used.is_empty() {
            println!("  (none)");
        }
        for (name, loc) in &used {
            println!(
                "  {} {}",
                style(format!("• {name}")).cyan(),
                style(loc).dim()
            );
        }
        if check && !used.is_empty() {
            std::process::exit(1);
        }
        return Ok(());
    }

    let missing: Vec<(&String, &String)> = used
        .iter()
        .filter(|(k, _)| !declared_set.contains(k.as_str()))
        .collect();
    let unused: Vec<&String> = declared
        .iter()
        .filter(|k| !used_set.contains(k.as_str()))
        .collect();

    if !missing.is_empty() {
        println!(
            "{}",
            style(format!(
                "Used in code but missing from {} ({}):",
                input.display(),
                missing.len()
            ))
            .yellow()
            .bold()
        );
        for (name, loc) in &missing {
            println!(
                "  {} {}",
                style(format!("• {name}")).yellow(),
                style(loc).dim()
            );
        }
        println!();
    }

    if !unused.is_empty() {
        println!(
            "{}",
            style(format!(
                "Declared in {} but not found in code ({}):",
                input.display(),
                unused.len()
            ))
            .dim()
            .bold()
        );
        for name in &unused {
            println!("  {}", style(format!("• {name}")).dim());
        }
        println!();
    }

    if missing.is_empty() && unused.is_empty() {
        println!(
            "{}",
            style(format!("✓ In sync — {} variable(s).", declared.len()))
                .green()
                .bold()
        );
    } else if check {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, uniquely-named temp dir for a test; removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("env-wizard-main-test-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explicit_input_always_wins() {
        let dir = TempDir::new("explicit-wins");
        std::fs::write(dir.0.join(".env.example"), "").unwrap();
        std::fs::write(dir.0.join("foo.txt"), "").unwrap();

        let resolved = resolve_input_path_from(&dir.0, Some(Path::new("foo.txt")));
        assert_eq!(resolved, PathBuf::from("foo.txt"));
    }

    #[test]
    fn alias_priority_prefers_conventional_default() {
        let dir = TempDir::new("alias-priority");
        std::fs::write(dir.0.join(".env.sample"), "").unwrap();
        assert_eq!(
            resolve_input_path_from(&dir.0, None),
            PathBuf::from(".env.sample")
        );

        std::fs::write(dir.0.join(".env.example"), "").unwrap();
        assert_eq!(
            resolve_input_path_from(&dir.0, None),
            PathBuf::from(".env.example")
        );
    }

    #[test]
    fn no_candidate_falls_back_to_conventional_default() {
        let dir = TempDir::new("no-candidate");
        assert_eq!(
            resolve_input_path_from(&dir.0, None),
            PathBuf::from(".env.example")
        );
    }

    #[test]
    fn missing_code_vars_diffs_against_known_keys() {
        let dir = TempDir::new("missing-code-vars");
        std::fs::write(
            dir.0.join("server.js"),
            "const port = process.env.FOO;\nconst secret = process.env.BAR;\n",
        )
        .unwrap();

        let known_keys: HashSet<String> = ["FOO".to_string()].into_iter().collect();
        let missing = missing_code_vars(&dir.0, &known_keys);

        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].key, "BAR");
        assert!(missing[0].description.starts_with("detected in code: "));
    }

    #[test]
    fn missing_code_vars_is_empty_when_everything_is_known() {
        let dir = TempDir::new("missing-code-vars-empty");
        std::fs::write(dir.0.join("server.js"), "const port = process.env.FOO;\n").unwrap();

        let known_keys: HashSet<String> = ["FOO".to_string()].into_iter().collect();
        let missing = missing_code_vars(&dir.0, &known_keys);

        assert!(missing.is_empty());
    }

    #[test]
    fn dotenv_alias_takes_priority_over_config_template() {
        let dir = TempDir::new("dotenv-priority");
        std::fs::write(dir.0.join(".env.example"), "").unwrap();
        std::fs::write(dir.0.join("config.example.toml"), "").unwrap();

        let resolved = resolve_input_from(&dir.0, None);
        assert_eq!(resolved.path, PathBuf::from(".env.example"));
        assert!(matches!(resolved.format, InputFormat::Dotenv));
    }

    #[test]
    fn single_config_template_is_used_when_no_dotenv_example_exists() {
        let dir = TempDir::new("single-config-template");
        std::fs::write(dir.0.join("config.example.toml"), "").unwrap();

        let resolved = resolve_input_from(&dir.0, None);
        assert_eq!(resolved.path, PathBuf::from("config.example.toml"));
        assert!(matches!(
            resolved.format,
            InputFormat::Config(config::Format::Toml)
        ));
    }

    #[test]
    fn ambiguous_config_templates_fall_back_to_dotenv_default() {
        let dir = TempDir::new("ambiguous-config-templates");
        std::fs::write(dir.0.join("config.example.toml"), "").unwrap();
        std::fs::write(dir.0.join("settings.example.yaml"), "").unwrap();

        let resolved = resolve_input_from(&dir.0, None);
        assert_eq!(resolved.path, PathBuf::from(".env.example"));
        assert!(matches!(resolved.format, InputFormat::Dotenv));
    }

    #[test]
    fn explicit_input_falls_back_to_extension_when_file_does_not_exist() {
        // These paths don't exist under ".", so detection can't read content
        // and falls back to the extension-only inference.
        let resolved = resolve_input_from(Path::new("."), Some(Path::new("config.example.toml")));
        assert!(matches!(
            resolved.format,
            InputFormat::Config(config::Format::Toml)
        ));

        let resolved = resolve_input_from(Path::new("."), Some(Path::new("settings.yaml")));
        assert!(matches!(
            resolved.format,
            InputFormat::Config(config::Format::Yaml)
        ));

        let resolved = resolve_input_from(Path::new("."), Some(Path::new(".env.local")));
        assert!(matches!(resolved.format, InputFormat::Dotenv));
    }

    #[test]
    fn explicit_input_content_overrides_a_misleading_extension() {
        let dir = TempDir::new("explicit-content-override");
        // Named .json, actually TOML.
        std::fs::write(
            dir.0.join("config.example.json"),
            "host = \"localhost\"\nport = 5432\n",
        )
        .unwrap();

        let resolved = resolve_input_from(&dir.0, Some(Path::new("config.example.json")));
        assert!(matches!(
            resolved.format,
            InputFormat::Config(config::Format::Toml)
        ));
    }

    #[test]
    fn explicit_input_detects_format_for_extensionless_template() {
        let dir = TempDir::new("explicit-extensionless");
        std::fs::write(
            dir.0.join("config.example"),
            "host: localhost\nport: 5432\n",
        )
        .unwrap();

        let resolved = resolve_input_from(&dir.0, Some(Path::new("config.example")));
        assert!(matches!(
            resolved.format,
            InputFormat::Config(config::Format::Yaml)
        ));
    }

    #[test]
    fn explicit_dotenv_named_file_is_never_content_sniffed() {
        let dir = TempDir::new("explicit-dotenv-guard");
        // Quoted values here also happen to parse as valid TOML, but a
        // dotenv-shaped filename must never be reclassified.
        std::fs::write(
            dir.0.join(".env.example"),
            "HOST=\"localhost\"\nPORT=\"5432\"\n",
        )
        .unwrap();

        let resolved = resolve_input_from(&dir.0, Some(Path::new(".env.example")));
        assert!(matches!(resolved.format, InputFormat::Dotenv));
    }

    #[test]
    fn find_config_template_detects_extensionless_template_from_content() {
        let dir = TempDir::new("find-extensionless");
        std::fs::write(dir.0.join("config.example"), "host = \"localhost\"\n").unwrap();

        let resolved = resolve_input_from(&dir.0, None);
        assert_eq!(resolved.path, PathBuf::from("config.example"));
        assert!(matches!(
            resolved.format,
            InputFormat::Config(config::Format::Toml)
        ));
    }

    #[test]
    fn find_config_template_content_overrides_a_misleading_extension() {
        let dir = TempDir::new("find-content-override");
        // Named .json, actually YAML.
        std::fs::write(dir.0.join("config.example.json"), "host: localhost\n").unwrap();

        let resolved = resolve_input_from(&dir.0, None);
        assert_eq!(resolved.path, PathBuf::from("config.example.json"));
        assert!(matches!(
            resolved.format,
            InputFormat::Config(config::Format::Yaml)
        ));
    }

    #[test]
    fn find_config_template_skips_dotenv_named_files_even_with_a_marker() {
        let dir = TempDir::new("find-skips-dotenv");
        // ".env.template" carries a marker segment ("template") but is
        // dotenv-shaped and must never be offered as a config template.
        std::fs::write(dir.0.join(".env.template"), "HOST=\"localhost\"\n").unwrap();

        assert!(matches!(find_config_template(&dir.0), FoundTemplates::None));
    }

    #[test]
    fn default_output_strips_template_marker() {
        let resolved = ResolvedInput {
            path: PathBuf::from("config.example.toml"),
            format: InputFormat::Config(config::Format::Toml),
        };
        assert_eq!(default_output_for(&resolved), PathBuf::from("config.toml"));

        let resolved = ResolvedInput {
            path: PathBuf::from(".env.example"),
            format: InputFormat::Dotenv,
        };
        assert_eq!(default_output_for(&resolved), PathBuf::from(".env"));
    }

    /// End-to-end: resolve -> collect -> prompt (accepting every default,
    /// exactly like `--yes`) -> apply -> write, for a TOML config template.
    /// Since nothing is changed, the written file must be byte-identical to
    /// the example — the same guarantee the per-format round-trip tests
    /// check in isolation, now exercised through the real `main()` plumbing.
    #[test]
    fn end_to_end_yes_run_on_config_template_is_byte_identical() {
        let dir = TempDir::new("end-to-end-toml");
        let example = "# Database connection\n[database]\nhost = \"localhost\"\nport = 5432\n";
        std::fs::write(dir.0.join("config.example.toml"), example).unwrap();

        let resolved = resolve_input_from(&dir.0, None);
        assert_eq!(resolved.path, PathBuf::from("config.example.toml"));

        let input_path = dir.0.join(&resolved.path);
        let source = collect_config_vars(&input_path, config::Format::Toml).unwrap();
        let prompt_vars = prompt_vars_for(&source);
        assert_eq!(prompt_vars.len(), 2);

        let opts = Options {
            no_ai: true,
            accept_defaults: true,
        };
        let answers = match prompt::run(&dir.0, &prompt_vars, &opts).unwrap() {
            Outcome::Completed(a) => a,
            Outcome::Quit => panic!("--yes-equivalent run should never quit"),
        };

        let Source::Config(mut doc, fields) = source else {
            panic!("expected a config source");
        };
        let values: Vec<String> = answers.into_iter().map(|(_, v)| v).collect();
        config::apply_answers(doc.as_mut(), &fields, &values).unwrap();

        let out_path = dir.0.join(default_output_for(&resolved));
        writer::write_file(&out_path, &doc.render(), true, false).unwrap();

        let written = std::fs::read_to_string(&out_path).unwrap();
        assert_eq!(written, example);
    }

    #[test]
    fn end_to_end_yes_run_on_dotenv_writes_expected_content() {
        let dir = TempDir::new("end-to-end-dotenv");
        let example =
            "# Port to listen on\nPORT=3000\n\n# Secret used to sign cookies\nSECRET_KEY=\n";
        std::fs::write(dir.0.join(".env.example"), example).unwrap();

        let resolved = resolve_input_from(&dir.0, None);
        assert_eq!(resolved.path, PathBuf::from(".env.example"));
        assert!(matches!(resolved.format, InputFormat::Dotenv));

        let cli = Cli {
            command: None,
            input: None,
            output: None,
            yes: true,
            no_ai: true,
            from_code: false,
        };
        let input_path = dir.0.join(&resolved.path);
        let prompt_vars = collect_dotenv_vars(&cli, &input_path, &dir.0).unwrap();
        assert_eq!(prompt_vars.len(), 2);

        let opts = Options {
            no_ai: cli.no_ai,
            accept_defaults: cli.yes,
        };
        let answers = match prompt::run(&dir.0, &prompt_vars, &opts).unwrap() {
            Outcome::Completed(a) => a,
            Outcome::Quit => panic!("--yes-equivalent run should never quit"),
        };

        let out_path = dir.0.join(default_output_for(&resolved));
        writer::write_env(&out_path, &answers, true).unwrap();

        let written = std::fs::read_to_string(&out_path).unwrap();
        assert_eq!(
            written,
            "# Port to listen on\nPORT=3000\n\n# Secret used to sign cookies\nSECRET_KEY=\n"
        );
    }
}
