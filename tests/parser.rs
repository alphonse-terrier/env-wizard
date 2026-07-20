//! Tests for the `.env.example` parser.

use env_wizard::parser::{parse, EnvVar};

#[test]
fn comment_becomes_description() {
    let vars = parse("# The database connection URL\nDATABASE_URL=");
    assert_eq!(
        vars,
        vec![EnvVar {
            key: "DATABASE_URL".into(),
            default: None,
            description: "The database connection URL".into(),
        }]
    );
}

#[test]
fn multi_line_comments_are_kept_on_separate_lines() {
    let vars = parse("# First line\n# Second line\nAPI_KEY=");
    assert_eq!(vars[0].description, "First line\nSecond line");
}

#[test]
fn default_value_is_captured() {
    let vars = parse("PORT=8080");
    assert_eq!(vars[0].default.as_deref(), Some("8080"));
}

#[test]
fn empty_value_is_none() {
    let vars = parse("SECRET=");
    assert_eq!(vars[0].default, None);
}

#[test]
fn export_prefix_is_stripped() {
    let vars = parse("export TOKEN=abc");
    assert_eq!(vars[0].key, "TOKEN");
    assert_eq!(vars[0].default.as_deref(), Some("abc"));
}

#[test]
fn quotes_are_removed() {
    let double = parse("MSG=\"hello world\"");
    assert_eq!(double[0].default.as_deref(), Some("hello world"));
    let single = parse("MSG='hello'");
    assert_eq!(single[0].default.as_deref(), Some("hello"));
}

#[test]
fn blank_line_resets_comment() {
    let vars = parse("# orphan comment\n\nKEY=value");
    assert_eq!(vars[0].description, "");
}

#[test]
fn order_is_preserved() {
    let vars = parse("A=1\nB=2\nC=3");
    let keys: Vec<_> = vars.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(keys, vec!["A", "B", "C"]);
}

#[test]
fn invalid_keys_are_ignored() {
    let vars = parse("1BAD=x\ngood_KEY=y");
    let keys: Vec<_> = vars.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(keys, vec!["good_KEY"]);
}
