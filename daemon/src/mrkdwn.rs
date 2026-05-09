//! GitHub-flavored Markdown → Slack mrkdwn conversion.
//!
//! Claude emits standard Markdown (`**bold**`, `# Heading`, `[label](url)`,
//! ```` ```lang ```` fences, `- item`). Slack uses its own [mrkdwn
//! flavor](https://api.slack.com/reference/surfaces/formatting), so the raw
//! text renders with literal asterisks, hashes, and broken links. This module
//! converts at the daemon's outbound boundary.
//!
//! Done code-side rather than via system prompt because the model drifts
//! turn-to-turn and tool output isn't under stylistic control. The converter
//! sees every byte that goes to Slack regardless of source.
//!
//! The converter is intentionally not perfect — it covers the cases the
//! model actually emits, not every Markdown edge case. Notably:
//! - `*x*` (single-asterisk italic) is rewritten to `_x_` only when the
//!   span looks deliberate (non-empty, no whitespace adjacent to delimiters,
//!   no embedded newline). Bare `*` characters in prose are left alone.
//! - `[label](url)` becomes `<url|label>`. URLs containing `)` get cut
//!   at the first `)`; markdown escapes (`\)`) aren't honored.
//! - Code spans (` `…` `, ```` ```…``` ````) are passed through unchanged
//!   so backtick contents never get rewritten.

/// Convert Claude's Markdown output into Slack mrkdwn.
pub fn to_slack_mrkdwn(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_fence = false;
    for line in s.split_inclusive('\n') {
        let nl_len = if line.ends_with('\n') { 1 } else { 0 };
        let body = &line[..line.len() - nl_len];
        let nl = &line[line.len() - nl_len..];
        if body.trim_start().starts_with("```") {
            // Fence delimiter — strip language tag on open, keep bare ``` on close.
            out.push_str("```");
            out.push_str(nl);
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        out.push_str(&convert_line(body));
        out.push_str(nl);
    }
    out
}

fn convert_line(line: &str) -> String {
    if let Some(text) = strip_heading(line) {
        // Slack has no heading syntax; render as bold. Strip any nested bold
        // markers so we don't emit `**Title**` (which renders ambiguously).
        let cleaned = text.replace("**", "").replace("__", "");
        let cleaned = cleaned.trim();
        if cleaned.is_empty() {
            return String::new();
        }
        return format!("*{}*", cleaned);
    }
    let line = convert_bullet(line);
    apply_inline(&line)
}

/// `# H1` / `## H2` / ... up to 6 `#`. Returns the text after the hashes,
/// or `None` if the line isn't a heading.
fn strip_heading(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut hashes = 0;
    while hashes < bytes.len() && bytes[hashes] == b'#' {
        hashes += 1;
    }
    if hashes == 0 || hashes > 6 {
        return None;
    }
    if hashes >= bytes.len() || bytes[hashes] != b' ' {
        return None;
    }
    Some(&line[hashes + 1..])
}

/// `- item` / `* item` / `+ item` (with optional leading whitespace) →
/// `<indent>• item`. Numbered lists are left alone; Slack renders them.
fn convert_bullet(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'-' || bytes[0] == b'*' || bytes[0] == b'+')
        && bytes[1] == b' '
    {
        format!("{}• {}", indent, &trimmed[2..])
    } else {
        line.to_string()
    }
}

/// Apply inline replacements while leaving code spans untouched. Splits on
/// matched backtick runs of equal length (Markdown's rule for code spans).
fn apply_inline(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let mut j = i;
            while j < bytes.len() && bytes[j] == b'`' {
                j += 1;
            }
            let run = j - i;
            // Search for a matching run of the same length on the same line.
            let mut k = j;
            let mut found_close = None;
            while k < bytes.len() {
                if bytes[k] == b'\n' {
                    break;
                }
                if bytes[k] == b'`' {
                    let mut m = k;
                    while m < bytes.len() && bytes[m] == b'`' {
                        m += 1;
                    }
                    if m - k == run {
                        found_close = Some((k, m));
                        break;
                    }
                    k = m;
                } else {
                    k += 1;
                }
            }
            if let Some((_, m)) = found_close {
                // Flush prose-so-far through the inline transformer; emit
                // the code span verbatim.
                out.push_str(&apply_inline_no_code(&s[cursor..i]));
                out.push_str(&s[i..m]);
                cursor = m;
                i = m;
                continue;
            }
            // Unmatched backtick(s) — fall through and treat as prose.
            i = j;
        } else {
            i += 1;
        }
    }
    out.push_str(&apply_inline_no_code(&s[cursor..]));
    out
}

/// Stand-in for `*` while bold spans are temporarily marked, so the italic
/// pass doesn't reinterpret the bold delimiter. U+0001 (Start of Heading)
/// is a control character that should never appear in model output.
const BOLD_SENTINEL: char = '\u{0001}';

fn apply_inline_no_code(s: &str) -> String {
    let bold_wrap = BOLD_SENTINEL.to_string();
    let s = replace_paired(s, "**", &bold_wrap);
    let s = replace_paired(&s, "__", &bold_wrap);
    let s = replace_paired(&s, "~~", "~");
    let s = replace_italic_single_asterisk(&s);
    let s = s.replace(BOLD_SENTINEL, "*");
    replace_links(&s)
}

/// Replace `<marker>X<marker>` with `<wrap>X<wrap>`. Unmatched markers are
/// emitted verbatim. Spans containing newlines are rejected (kept literal).
fn replace_paired(s: &str, marker: &str, wrap: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find(marker) {
        out.push_str(&rest[..start]);
        let after = &rest[start + marker.len()..];
        if let Some(end) = after.find(marker) {
            let content = &after[..end];
            if !content.is_empty() && !content.contains('\n') {
                out.push_str(wrap);
                out.push_str(content);
                out.push_str(wrap);
                rest = &after[end + marker.len()..];
                continue;
            }
        }
        out.push_str(marker);
        rest = after;
    }
    out.push_str(rest);
    out
}

/// After `**bold**` and `__bold__` are collapsed to `*bold*`, any remaining
/// `*X*` is GitHub italic. Convert to Slack `_X_`. Leaves stray `*` alone.
fn replace_italic_single_asterisk(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'*' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() && bytes[j] != b'*' && bytes[j] != b'\n' {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'*' || j == i + 1 {
            i += 1;
            continue;
        }
        let inner = &s[i + 1..j];
        let first = inner.chars().next();
        let last = inner.chars().last();
        let bounded = first.is_some_and(|c| !c.is_whitespace())
            && last.is_some_and(|c| !c.is_whitespace());
        if !bounded {
            i += 1;
            continue;
        }
        out.push_str(&s[cursor..i]);
        out.push('_');
        out.push_str(inner);
        out.push('_');
        cursor = j + 1;
        i = j + 1;
    }
    out.push_str(&s[cursor..]);
    out
}

/// `[label](url)` → `<url|label>`. URLs ending at the first `)` — escapes
/// (`\)`) and nested parens aren't honored. If the parse fails, the literal
/// `[...]` text is kept and the converter moves on.
fn replace_links(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close_b) = s[i + 1..].find(']') {
                let label_end = i + 1 + close_b;
                if bytes.get(label_end + 1) == Some(&b'(') {
                    if let Some(close_p) = s[label_end + 2..].find(')') {
                        let url_end = label_end + 2 + close_p;
                        let label = &s[i + 1..label_end];
                        let url = &s[label_end + 2..url_end];
                        // Reject if either side contains a newline — it's
                        // probably not a link.
                        if !label.contains('\n') && !url.contains('\n') {
                            out.push_str(&s[cursor..i]);
                            out.push('<');
                            out.push_str(url);
                            out.push('|');
                            out.push_str(label);
                            out.push('>');
                            cursor = url_end + 1;
                            i = url_end + 1;
                            continue;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    out.push_str(&s[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_double_asterisk() {
        assert_eq!(to_slack_mrkdwn("**hello**"), "*hello*");
        assert_eq!(to_slack_mrkdwn("a **b** c"), "a *b* c");
    }

    #[test]
    fn bold_double_underscore() {
        assert_eq!(to_slack_mrkdwn("__hello__"), "*hello*");
    }

    #[test]
    fn italic_single_asterisk() {
        assert_eq!(to_slack_mrkdwn("*emph*"), "_emph_");
        assert_eq!(to_slack_mrkdwn("a *b* c"), "a _b_ c");
    }

    #[test]
    fn bold_plus_italic_in_one_line() {
        assert_eq!(to_slack_mrkdwn("**bold** and *italic*"), "*bold* and _italic_");
    }

    #[test]
    fn strikethrough() {
        assert_eq!(to_slack_mrkdwn("~~gone~~"), "~gone~");
    }

    #[test]
    fn headings() {
        assert_eq!(to_slack_mrkdwn("# Title"), "*Title*");
        assert_eq!(to_slack_mrkdwn("### Sub"), "*Sub*");
        assert_eq!(to_slack_mrkdwn("####### too many"), "####### too many");
    }

    #[test]
    fn link() {
        assert_eq!(
            to_slack_mrkdwn("[click](https://example.com)"),
            "<https://example.com|click>"
        );
    }

    #[test]
    fn bullet_dash() {
        assert_eq!(to_slack_mrkdwn("- one\n- two"), "• one\n• two");
    }

    #[test]
    fn bullet_asterisk_does_not_become_italic() {
        // The bullet `*` must not be eaten by the italic pass.
        assert_eq!(to_slack_mrkdwn("* one\n* two"), "• one\n• two");
    }

    #[test]
    fn code_span_is_preserved() {
        // The `**` inside backticks must not be rewritten.
        assert_eq!(
            to_slack_mrkdwn("use `**raw**` like this"),
            "use `**raw**` like this"
        );
    }

    #[test]
    fn fenced_code_block_passes_through_with_lang_stripped() {
        let input = "```rust\nlet x = **42**;\n```";
        let expected = "```\nlet x = **42**;\n```";
        assert_eq!(to_slack_mrkdwn(input), expected);
    }

    #[test]
    fn multiline_with_mixed_content() {
        let input = "# Hi\n\n**bold** and [link](https://x.io)\n- a\n- b";
        let expected = "*Hi*\n\n*bold* and <https://x.io|link>\n• a\n• b";
        assert_eq!(to_slack_mrkdwn(input), expected);
    }

    #[test]
    fn bare_asterisk_left_alone() {
        // No closing `*` on the line, so don't italicize anything.
        assert_eq!(to_slack_mrkdwn("a*b c"), "a*b c");
    }

    #[test]
    fn unbalanced_bold_passes_through() {
        assert_eq!(to_slack_mrkdwn("**unclosed"), "**unclosed");
    }

    #[test]
    fn italic_skips_whitespace_adjacency() {
        // Inline `* foo *` shouldn't become italic — the inner span has
        // whitespace adjacent to both delimiters. (Note: `* foo *` at the
        // start of a line is handled by the bullet pass before italic runs.)
        assert_eq!(to_slack_mrkdwn("a * foo * b"), "a * foo * b");
    }

    #[test]
    fn empty_string() {
        assert_eq!(to_slack_mrkdwn(""), "");
    }

    #[test]
    fn unicode_in_bold() {
        assert_eq!(to_slack_mrkdwn("**café**"), "*café*");
    }
}