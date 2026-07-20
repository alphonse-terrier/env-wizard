//! Tests for provider config (de)serialization and path resolution.
//!
//! These mutate process-global environment variables and touch a scratch file,
//! so they run in a single serialized test to avoid cross-test interference.

use std::path::PathBuf;

use env_wizard::provider::{self, CommandProvider, Config, OpenaiProvider};

fn command_config() -> Config {
    Config {
        kind: "command".into(),
        label: "Claude (CLI)".into(),
        command: Some(CommandProvider {
            program: "claude".into(),
            args: vec!["-p".into()],
            prompt_via: "arg".into(),
        }),
        openai: None,
    }
}

fn openai_config() -> Config {
    Config {
        kind: "openai".into(),
        label: "OpenAI/gpt-4o-mini".into(),
        command: None,
        openai: Some(OpenaiProvider {
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o-mini".into(),
            api_key_env: "OPENAI_API_KEY".into(),
        }),
    }
}

#[test]
fn config_roundtrip_and_path_resolution() {
    let dir = std::env::temp_dir().join("env-wizard-test-provider");
    let _ = std::fs::create_dir_all(&dir);
    let cfg_path = dir.join("config.toml");

    // `config_path()` honours $ENV_WIZARD_CONFIG.
    std::env::set_var("ENV_WIZARD_CONFIG", &cfg_path);
    assert_eq!(provider::config_path(), PathBuf::from(&cfg_path));

    // Round-trip: a command config saves and loads back identically.
    let _ = std::fs::remove_file(&cfg_path);
    assert!(provider::load().unwrap().is_none());
    let saved = provider::save(&command_config()).unwrap();
    assert_eq!(saved, cfg_path);
    assert_eq!(provider::load().unwrap(), Some(command_config()));

    // Round-trip: an openai config too.
    provider::save(&openai_config()).unwrap();
    assert_eq!(provider::load().unwrap(), Some(openai_config()));

    std::env::remove_var("ENV_WIZARD_CONFIG");
    let _ = std::fs::remove_file(&cfg_path);
}
