#[derive(Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Ordered,
    Unordered,
}

pub fn to_html(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut idx = 0;
    let mut html = String::new();

    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            idx += 1;
            continue;
        }

        if let Some((kind, items, next_idx)) = parse_list(&lines, idx) {
            match kind {
                ListKind::Ordered => html.push_str("<ol>"),
                ListKind::Unordered => html.push_str("<ul>"),
            }
            for item in items {
                html.push_str("<li>");
                html.push_str(&render_inline(&item));
                html.push_str("</li>");
            }
            match kind {
                ListKind::Ordered => html.push_str("</ol>"),
                ListKind::Unordered => html.push_str("</ul>"),
            }
            idx = next_idx;
            continue;
        }

        let (paragraph, next_idx) = parse_paragraph(&lines, idx);
        if !paragraph.is_empty() {
            html.push_str("<p>");
            html.push_str(&render_inline(&paragraph));
            html.push_str("</p>");
        }
        idx = next_idx;
    }

    if html.is_empty() {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let escaped = render_inline(trimmed);
        return format!("<p>{escaped}</p>");
    }

    html
}

fn parse_list(lines: &[&str], start: usize) -> Option<(ListKind, Vec<String>, usize)> {
    let (kind, first) = detect_list_marker(lines[start])?;
    let mut items = vec![first.trim().to_string()];
    let mut idx = start + 1;

    while idx < lines.len() {
        let current = lines[idx];
        let trimmed = current.trim();
        if trimmed.is_empty() {
            idx += 1;
            continue;
        }

        if let Some((candidate_kind, content)) = detect_list_marker(current) {
            if candidate_kind == kind {
                items.push(content.trim().to_string());
                idx += 1;
                continue;
            }
        }
        let starts_with_indent = current.starts_with(' ') || current.starts_with('\t');
        if starts_with_indent {
            if let Some(last) = items.last_mut() {
                if !trimmed.is_empty() {
                    last.push(' ');
                    last.push_str(trimmed);
                }
                idx += 1;
                continue;
            }
        }
        break;
    }

    Some((kind, items, idx))
}

fn parse_paragraph(lines: &[&str], start: usize) -> (String, usize) {
    let mut content = Vec::new();
    let mut idx = start;

    while idx < lines.len() {
        let line = lines[idx];
        if line.trim().is_empty() {
            idx += 1;
            break;
        }
        if detect_list_marker(line).is_some() {
            break;
        }
        content.push(line.trim());
        idx += 1;
    }

    (content.join(" "), idx)
}

fn detect_list_marker(line: &str) -> Option<(ListKind, String)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
        let content = trimmed[2..].to_string();
        return Some((ListKind::Unordered, content));
    }

    if let Some((digits_len, suffix_len)) = ordered_marker_lengths(trimmed) {
        let start = digits_len + suffix_len;
        let content = trimmed[start..].to_string();
        return Some((ListKind::Ordered, content));
    }

    if trimmed.starts_with("• ") {
        let content = trimmed[2..].to_string();
        return Some((ListKind::Unordered, content));
    }

    None
}

fn ordered_marker_lengths(text: &str) -> Option<(usize, usize)> {
    let mut digits_len = 0;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits_len += ch.len_utf8();
        } else {
            break;
        }
    }

    if digits_len == 0 || digits_len == text.len() {
        return None;
    }

    let rest = &text[digits_len..];
    if rest.starts_with(". ") {
        return Some((digits_len, 2));
    }
    if rest.starts_with(") ") {
        return Some((digits_len, 2));
    }

    None
}

fn render_inline(text: &str) -> String {
    let escaped = escape_html(text);
    apply_bold(&escaped)
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '&' => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn apply_bold(text: &str) -> String {
    let mut result = String::new();
    let mut remainder = text;
    let mut open = false;

    while let Some(pos) = remainder.find("**") {
        let (before, after) = remainder.split_at(pos);
        result.push_str(before);
        result.push_str(if open { "</strong>" } else { "<strong>" });
        remainder = &after[2..];
        open = !open;
    }

    result.push_str(remainder);
    if open {
        result.push_str("</strong>");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_paragraphs_and_lists() {
        let input = "Intro line.\n1. First item\n2. Second item\n\n- Extra\n- More";
        let html = to_html(input);
        assert!(html.contains("<p>Intro line.</p>"));
        assert!(html.contains("<ol>"));
        assert!(html.contains("<li>First item</li>"));
        assert!(html.contains("<li>Second item</li>"));
        assert!(html.contains("<ul>"));
        assert!(html.contains("<li>Extra</li>"));
        assert!(html.contains("<li>More</li>"));
    }

    #[test]
    fn converts_bold_markers() {
        let input = "**Bold** and normal.";
        let html = to_html(input);
        assert!(html.contains("<strong>Bold</strong>"));
        assert!(html.contains("and normal."));
    }

    #[test]
    fn escapes_html() {
        let input = "<script>alert(1)</script>";
        let html = to_html(input);
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    }
}
