//! Minimal Markdown → terminal renderer for AI hints.
//!
//! LLMs answer in Markdown, which looks noisy in a terminal (`**bold**`,
//! `` `code` ``, `#` headings, ``` fences). This turns the common constructs
//! into styled terminal text using `console`, and drops the syntax markers.
//! When colors are disabled (e.g. piped output, `NO_COLOR`) the result is clean
//! plain text.

use console::style;

/// Renders a Markdown string into styled, terminal-friendly text.
pub fn markdown_to_terminal(input: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_code_block = false;

    for line in input.lines() {
        let trimmed = line.trim_end();
        let stripped = trimmed.trim_start();

        // Fenced code blocks: drop the ``` fences, style the body.
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            out.push(format!("    {}", style(trimmed).green()));
            continue;
        }

        // Headings: `# Title` → bold underlined, marker removed.
        if let Some(title) = strip_heading(stripped) {
            out.push(style(render_inline(title)).bold().underlined().to_string());
            continue;
        }

        // Blockquote: `> quote` → dim, marker removed.
        if let Some(quote) = stripped.strip_prefix("> ") {
            out.push(format!("  {}", style(render_inline(quote)).dim()));
            continue;
        }

        // List item: `- x` / `* x` / `+ x` → `• x` (indent preserved).
        if let Some((indent, content)) = split_bullet(trimmed) {
            out.push(format!(
                "{indent}{} {}",
                style("•").cyan(),
                render_inline(content)
            ));
            continue;
        }

        out.push(render_inline(trimmed));
    }

    out.join("\n")
}

/// Returns the heading text if the line is an ATX heading (`#`..`######`).
fn strip_heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &line[hashes..];
        if let Some(text) = rest.strip_prefix(' ') {
            return Some(text.trim());
        }
    }
    None
}

/// Splits a bullet line into `(leading_indent, content)` if it is one.
fn split_bullet(line: &str) -> Option<(String, &str)> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let rest = &line[indent_len..];
    for marker in ["- ", "* ", "+ "] {
        if let Some(content) = rest.strip_prefix(marker) {
            return Some((indent.to_string(), content));
        }
    }
    None
}

/// Caps recursion in [`render_inline_depth`] for nested `**bold**` spans. An AI
/// response's shape/length isn't bounded, so without a cap a pathological
/// (deeply or repeatedly nested) response could overflow the stack.
const MAX_INLINE_DEPTH: usize = 32;

/// Renders inline spans: `` `code` ``, `**bold**`/`__bold__`, `*italic*`/`_italic_`.
fn render_inline(s: &str) -> String {
    render_inline_depth(s, 0)
}

fn render_inline_depth(s: &str, depth: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // Inline code.
        if c == '`' {
            if let Some(end) = find_single(&chars, i + 1, '`') {
                let code: String = chars[i + 1..end].iter().collect();
                result.push_str(&style(code).yellow().to_string());
                i = end + 1;
                continue;
            }
        }

        // Bold: ** ** or __ __ (checked before single-char emphasis).
        if (c == '*' || c == '_') && i + 1 < chars.len() && chars[i + 1] == c {
            if let Some(end) = find_double(&chars, i + 2, c) {
                let inner: String = chars[i + 2..end].iter().collect();
                let rendered = if depth < MAX_INLINE_DEPTH {
                    render_inline_depth(&inner, depth + 1)
                } else {
                    inner
                };
                result.push_str(&style(rendered).bold().to_string());
                i = end + 2;
                continue;
            }
        }

        // Italic: * * or _ _.
        if c == '*' || c == '_' {
            if let Some(end) = find_single(&chars, i + 1, c) {
                let inner: String = chars[i + 1..end].iter().collect();
                result.push_str(&style(inner).italic().to_string());
                i = end + 1;
                continue;
            }
        }

        result.push(c);
        i += 1;
    }

    result
}

/// Index of the next single occurrence of `target` at or after `from`.
fn find_single(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&j| chars[j] == target)
}

/// Index of the next doubled `target` (`target target`) at or after `from`.
fn find_double(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len().saturating_sub(1)).find(|&j| chars[j] == target && chars[j + 1] == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Colors off → assertions see the structural transform, not ANSI codes.
    fn plain(s: &str) -> String {
        console::set_colors_enabled(false);
        markdown_to_terminal(s)
    }

    #[test]
    fn strips_bold_and_code_markers() {
        assert_eq!(
            plain("Use **openssl** and `rand -hex 32`"),
            "Use openssl and rand -hex 32"
        );
    }

    #[test]
    fn heading_marker_removed() {
        assert_eq!(plain("## Setup"), "Setup");
    }

    #[test]
    fn deeply_nested_bold_does_not_overflow_the_stack() {
        // Alternate `**`/`__` markers per level so `find_double` (which only
        // matches the exact marker char that opened a span) can't prematurely
        // close an outer span on an inner level's markers — this constructs
        // genuinely nested spans, not adjacent same-char runs that collapse
        // into empty pairs.
        let mut input = "core".to_string();
        for i in 0..200 {
            let marker = if i % 2 == 0 { "**" } else { "__" };
            input = format!("{marker}{input}{marker}");
        }
        // Just needs to return without panicking/overflowing the stack; exact
        // styling beyond the recursion cap isn't the point of this test.
        let out = plain(&input);
        assert!(out.contains("core"));
    }

    #[test]
    fn code_fences_dropped_content_kept() {
        assert_eq!(
            plain("```sh\nopenssl rand -hex 32\n```"),
            "    openssl rand -hex 32"
        );
    }

    #[test]
    fn bullets_become_dots() {
        assert_eq!(plain("- one\n- two"), "• one\n• two");
    }

    #[test]
    fn nested_bold_inside_bullet() {
        assert_eq!(plain("- set **PORT** now"), "• set PORT now");
    }

    #[test]
    fn blockquote_marker_removed() {
        assert_eq!(plain("> note this"), "  note this");
    }

    #[test]
    fn unmatched_markers_left_literal() {
        // A lone backtick / asterisk with no closing pair stays as-is.
        assert_eq!(plain("use ` carefully"), "use ` carefully");
        assert_eq!(plain("2 * 3 = 6"), "2 * 3 = 6");
    }
}
