//! Log text sanitizer: a verbatim port of the original server (`server/src/main.rs:488-604`),
//! quirks included (case-sensitive prefixes, no word boundary, redaction can lengthen the text).

/// Stored questions and answers keep at most this many characters (plus the truncation suffix).
pub const MAX_LOG_TEXT_CHARS: usize = 2000;
/// Stored command lines keep at most this many characters (plus the truncation suffix).
pub const MAX_COMMAND_LOG_CHARS: usize = 200;

/// `truncate(redact(normalize(s)), 2000)`: questions and answers.
pub fn sanitize_log_text(input: &str) -> String {
    sanitize_with_limit(input, MAX_LOG_TEXT_CHARS)
}

/// `truncate(redact(normalize(s)), 200)`: command lines.
pub fn sanitize_command(input: &str) -> String {
    sanitize_with_limit(input, MAX_COMMAND_LOG_CHARS)
}

fn sanitize_with_limit(input: &str, max_chars: usize) -> String {
    let normalized = normalize_log_text(input);
    let redacted = redact_known_secret_patterns(&normalized);
    truncate_for_log(&redacted, max_chars)
}

/// Control characters become spaces, whitespace runs collapse to one space, then trim.
fn normalize_log_text(input: &str) -> String {
    let mut normalized = String::with_capacity(input.len());
    let mut last_was_space = false;

    for ch in input.chars() {
        let mapped = if ch.is_control() { ' ' } else { ch };
        if mapped.is_whitespace() {
            if !last_was_space {
                normalized.push(' ');
                last_was_space = true;
            }
        } else {
            normalized.push(mapped);
            last_was_space = false;
        }
    }

    normalized.trim().to_string()
}

fn redact_known_secret_patterns(input: &str) -> String {
    let mut redacted = input.to_string();
    redacted = redact_prefixed_secret(&redacted, "sk-proj-", "[redacted-openai-key]", 12);
    redacted = redact_prefixed_secret(&redacted, "sk-", "[redacted-openai-key]", 20);
    redacted = redact_prefixed_secret(&redacted, "gsk_", "[redacted-groq-key]", 12);
    redacted = redact_prefixed_secret(&redacted, "pcsk_", "[redacted-pinecone-key]", 12);
    redacted = redact_prefixed_secret(&redacted, "AIza", "[redacted-google-key]", 12);
    redact_bearer_token(&redacted)
}

fn redact_prefixed_secret(
    input: &str,
    prefix: &str,
    replacement: &str,
    min_secret_tail_len: usize,
) -> String {
    let mut redacted = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(index) = remaining.find(prefix) {
        let Some((before, candidate)) = remaining.split_at_checked(index) else {
            break;
        };
        redacted.push_str(before);

        let mut end = prefix.len();
        for ch in candidate.get(prefix.len()..).unwrap_or_default().chars() {
            if is_secret_char(ch) {
                end += ch.len_utf8();
            } else {
                break;
            }
        }

        if end.saturating_sub(prefix.len()) >= min_secret_tail_len {
            redacted.push_str(replacement);
        } else {
            redacted.push_str(candidate.get(..end).unwrap_or(candidate));
        }

        remaining = candidate.get(end..).unwrap_or_default();
    }

    redacted.push_str(remaining);
    redacted
}

fn redact_bearer_token(input: &str) -> String {
    let marker = "Bearer ";
    let mut redacted = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(index) = remaining.find(marker) {
        let Some((before, after_marker)) = remaining.split_at_checked(index) else {
            break;
        };
        redacted.push_str(before);
        redacted.push_str(marker);

        let token = after_marker.get(marker.len()..).unwrap_or_default();
        let mut end = 0;
        for ch in token.chars() {
            if is_secret_char(ch) {
                end += ch.len_utf8();
            } else {
                break;
            }
        }

        if end >= 12 {
            redacted.push_str("[redacted-bearer-token]");
        } else {
            redacted.push_str(token.get(..end).unwrap_or(token));
        }

        remaining = token.get(end..).unwrap_or_default();
    }

    redacted.push_str(remaining);
    redacted
}

fn truncate_for_log(input: &str, max_chars: usize) -> String {
    let char_count = input.chars().count();
    if char_count <= max_chars {
        return input.to_string();
    }

    let truncated = input.chars().take(max_chars).collect::<String>();
    format!("{truncated} [truncated {} chars]", char_count - max_chars)
}

fn is_secret_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ported from the original `log_redaction_and_cache_behavior` (the cache-control half went
    /// away with the static fallback).
    #[test]
    fn log_redaction_and_cache_behavior() {
        let output = sanitize_log_text(
            "hey\n sk-abcdefghijklmnopqrstuvwxyz Bearer abcdefghijklmnopqrstuvwxyz",
        );
        assert!(!output.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!output.contains('\n'));
    }

    /// The table of spec §9, checked against a faithful re-implementation of the original.
    #[test]
    fn spec_table() {
        let cases = [
            (
                "hey\n sk-abcdefghijklmnopqrstuvwxyz Bearer abcdefghijklmnopqrstuvwxyz",
                "hey [redacted-openai-key] Bearer [redacted-bearer-token]".to_string(),
            ),
            (
                "my key sk-proj-abc123DEF456ghi and sk-short",
                "my key [redacted-openai-key] and sk-short".to_string(),
            ),
            (
                "the task-abcdefghijklmnopqrstuvwxyz0 failed",
                "the ta[redacted-openai-key] failed".to_string(),
            ),
            (
                "Authorization: Bearer sk-abcdefghijklmnopqrstuvwx",
                "Authorization: Bearer [redacted-openai-key]".to_string(),
            ),
            (
                "bearer abcdefghijklmnopq",
                "bearer abcdefghijklmnopq".to_string(),
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(sanitize_log_text(input), expected, "input {input:?}");
        }
        let long = "x".repeat(2005);
        assert_eq!(
            sanitize_log_text(&long),
            format!("{} [truncated 5 chars]", "x".repeat(2000))
        );
    }

    #[test]
    fn other_prefixes_and_whitespace() {
        assert_eq!(
            sanitize_log_text("a gsk_abcdefghijkl b pcsk_abcdefghijkl c AIzaabcdefghijkl"),
            "a [redacted-groq-key] b [redacted-pinecone-key] c [redacted-google-key]"
        );
        // Short tails stay verbatim and the scan continues after them.
        assert_eq!(
            sanitize_log_text("gsk_short gsk_abcdefghijkl"),
            "gsk_short [redacted-groq-key]"
        );
        // NBSP and control characters collapse; zero-width characters stay.
        assert_eq!(
            sanitize_log_text("  a\u{a0}\u{a0}b\u{0}\tc\u{200b}d  "),
            "a b c\u{200b}d"
        );
        assert_eq!(sanitize_log_text("é😀 Bearer short"), "é😀 Bearer short");
        // Redaction can lengthen the text: AIza + 12 characters (16) becomes 21 characters.
        assert_eq!(sanitize_log_text("AIzaabcdefghijkl").chars().count(), 21);
    }

    #[test]
    fn command_cap_is_200_characters() {
        let command = "é".repeat(250);
        assert_eq!(
            sanitize_command(&command),
            format!("{} [truncated 50 chars]", "é".repeat(200))
        );
        assert_eq!(sanitize_command("projects"), "projects");
        assert_eq!(sanitize_command(&"y".repeat(200)), "y".repeat(200));
    }
}
