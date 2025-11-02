use crate::utils;
use futures::{pin_mut, stream, StreamExt};
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{Blob, Request, RequestInit, RequestMode, Response, Url};

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Text(String),
    Icon(IconMatch),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IconMatch {
    pub token: String,
    pub icon_path: &'static str,
}

#[derive(Debug)]
struct KeywordPattern {
    #[allow(dead_code)]
    pattern: &'static str,
    pattern_lower: &'static str,
    icon_path: &'static str,
}

const ICON_PRELOAD_CONCURRENCY: usize = 4;

const KEYWORD_PATTERNS: &[KeywordPattern] = &[
    KeywordPattern {
        pattern: "Amazon Web Services",
        pattern_lower: "amazon web services",
        icon_path: "/icons/amazonwebservices-original-wordmark.svg",
    },
    KeywordPattern {
        pattern: "Google Cloud Platform",
        pattern_lower: "google cloud platform",
        icon_path: "/icons/googlecloud-original.svg",
    },
    KeywordPattern {
        pattern: "GitHub Actions",
        pattern_lower: "github actions",
        icon_path: "/icons/githubactions-original.svg",
    },
    KeywordPattern {
        pattern: "Visual Studio",
        pattern_lower: "visual studio",
        icon_path: "/icons/visualstudio-original.svg",
    },
    KeywordPattern {
        pattern: "Google Cloud",
        pattern_lower: "google cloud",
        icon_path: "/icons/googlecloud-original.svg",
    },
        KeywordPattern {
        pattern: "Google",
        pattern_lower: "google",
        icon_path: "/icons/google-original.svg",
    },
    KeywordPattern {
        pattern: "AWS Lambda",
        pattern_lower: "aws lambda",
        icon_path: "/icons/amazonwebservices-original-wordmark.svg",
    },
    KeywordPattern {
        pattern: "GitLab CI",
        pattern_lower: "gitlab ci",
        icon_path: "/icons/gitlab-original.svg",
    },
    KeywordPattern {
        pattern: "Unreal Engine 5",
        pattern_lower: "unreal engine 5",
        icon_path: "/icons/unrealengine-original.svg",
    },
    KeywordPattern {
        pattern: "Slack API",
        pattern_lower: "slack api",
        icon_path: "/icons/slack-original.svg",
    },
    KeywordPattern {
        pattern: "Unreal Engine",
        pattern_lower: "unreal engine",
        icon_path: "/icons/unrealengine-original.svg",
    },
    KeywordPattern {
        pattern: "Node.js",
        pattern_lower: "node.js",
        icon_path: "/icons/nodejs-original.svg",
    },
    KeywordPattern {
        pattern: "NodeJS",
        pattern_lower: "nodejs",
        icon_path: "/icons/nodejs-original.svg",
    },
    KeywordPattern {
        pattern: "TypeScript",
        pattern_lower: "typescript",
        icon_path: "/icons/typescript-original.svg",
    },
    KeywordPattern {
        pattern: "JavaScript",
        pattern_lower: "javascript",
        icon_path: "/icons/javascript-original.svg",
    },
    KeywordPattern {
        pattern: "Kubernetes",
        pattern_lower: "kubernetes",
        icon_path: "/icons/kubernetes-original.svg",
    },
    KeywordPattern {
        pattern: "Figma",
        pattern_lower: "figma",
        icon_path: "/icons/figma-original.svg",
    },
    KeywordPattern {
        pattern: "Datadog",
        pattern_lower: "datadog",
        icon_path: "/icons/datadog-original.svg",
    },
    KeywordPattern {
        pattern: "Firebase",
        pattern_lower: "firebase",
        icon_path: "/icons/firebase-original.svg",
    },
    KeywordPattern {
        pattern: "Confluence",
        pattern_lower: "confluence",
        icon_path: "/icons/confluence-original.svg",
    },
    KeywordPattern {
        pattern: "Grafana",
        pattern_lower: "grafana",
        icon_path: "/icons/grafana-original.svg",
    },
    KeywordPattern {
        pattern: "Android",
        pattern_lower: "android",
        icon_path: "/icons/android-original.svg",
    },
    KeywordPattern {
        pattern: "Docker",
        pattern_lower: "docker",
        icon_path: "/icons/docker-original.svg",
    },
    KeywordPattern {
        pattern: "GitHub",
        pattern_lower: "github",
        icon_path: "/icons/github-original.svg",
    },
    KeywordPattern {
        pattern: "Azure",
        pattern_lower: "azure",
        icon_path: "/icons/azure-original.svg",
    },
    KeywordPattern {
        pattern: "Python",
        pattern_lower: "python",
        icon_path: "/icons/python-original.svg",
    },
    KeywordPattern {
        pattern: "GitLab",
        pattern_lower: "gitlab",
        icon_path: "/icons/gitlab-original.svg",
    },
    KeywordPattern {
        pattern: "Jira",
        pattern_lower: "jira",
        icon_path: "/icons/jira-original.svg",
    },
    KeywordPattern {
        pattern: "Jupyter Notebook",
        pattern_lower: "jupyter notebook",
        icon_path: "/icons/jupyter-original-wordmark.svg",
    },
    KeywordPattern {
        pattern: "Unity",
        pattern_lower: "unity",
        icon_path: "/icons/unity-original.svg",
    },
    KeywordPattern {
        pattern: "Unreal",
        pattern_lower: "unreal",
        icon_path: "/icons/unrealengine-original.svg",
    },
    KeywordPattern {
        pattern: "Slack",
        pattern_lower: "slack",
        icon_path: "/icons/slack-original.svg",
    },
    KeywordPattern {
        pattern: "Discord",
        pattern_lower: "discord",
        icon_path: "/icons/discord.svg",
    },
    KeywordPattern {
        pattern: "Discord API",
        pattern_lower: "discord api",
        icon_path: "/icons/discord.svg",
    },
    KeywordPattern {
        pattern: "Discord Bot",
        pattern_lower: "discord bot",
        icon_path: "/icons/discord.svg",
    },
    KeywordPattern {
        pattern: "Twitch",
        pattern_lower: "twitch",
        icon_path: "/icons/twitch.svg",
    },
    KeywordPattern {
        pattern: "Twitch API",
        pattern_lower: "twitch api",
        icon_path: "/icons/twitch.svg",
    },
    KeywordPattern {
        pattern: "Twitch Bot",
        pattern_lower: "twitch bot",
        icon_path: "/icons/twitch.svg",
    },
    KeywordPattern {
        pattern: "PlayStation",
        pattern_lower: "playstation",
        icon_path: "/icons/playstation.svg",
    },
    KeywordPattern {
        pattern: "PlayStation 5",
        pattern_lower: "playstation 5",
        icon_path: "/icons/playstation.svg",
    },
    KeywordPattern {
        pattern: "PS5",
        pattern_lower: "ps5",
        icon_path: "/icons/playstation.svg",
    },
    KeywordPattern {
        pattern: "LinkedIn",
        pattern_lower: "linkedin",
        icon_path: "/icons/linkedin-original.svg",
    },
    KeywordPattern {
        pattern: "Linear",
        pattern_lower: "linear",
        icon_path: "/icons/linear-original.svg",
    },
    KeywordPattern {
        pattern: "Alexandre DO-O ALMEIDA",
        pattern_lower: "alexandre do-o almeida",
        icon_path: "/images/alexandre.webp",
    },
    KeywordPattern {
        pattern: "Meta Platforms",
        pattern_lower: "meta platforms",
        icon_path: "/icons/meta-original.svg",
    },
    KeywordPattern {
        pattern: "Meta",
        pattern_lower: "meta",
        icon_path: "/icons/meta-original.svg",
    },
    KeywordPattern {
        pattern: "Y Combinator",
        pattern_lower: "y combinator",
        icon_path: "/icons/ycombinator.svg",
    },
    KeywordPattern {
        pattern: "YC",
        pattern_lower: "yc",
        icon_path: "/icons/ycombinator.svg",
    },
    KeywordPattern {
        pattern: "AWS",
        pattern_lower: "aws",
        icon_path: "/icons/amazonwebservices-original-wordmark.svg",
    },
    KeywordPattern {
        pattern: "GCP",
        pattern_lower: "gcp",
        icon_path: "/icons/googlecloud-original.svg",
    },
    KeywordPattern {
        pattern: "Rust",
        pattern_lower: "rust",
        icon_path: "/icons/rust-original.svg",
    },
    KeywordPattern {
        pattern: "React",
        pattern_lower: "react",
        icon_path: "/icons/react-original.svg",
    },
    KeywordPattern {
        pattern: "Go",
        pattern_lower: "go",
        icon_path: "/icons/go-original.svg",
    },
    KeywordPattern {
        pattern: "Java",
        pattern_lower: "java",
        icon_path: "/icons/java-original.svg",
    },
    KeywordPattern {
        pattern: "Lua",
        pattern_lower: "lua",
        icon_path: "/icons/lua-original.svg",
    },
    KeywordPattern {
        pattern: "Maya",
        pattern_lower: "maya",
        icon_path: "/icons/maya-original.svg",
    },
    KeywordPattern {
        pattern: "SQL",
        pattern_lower: "sql",
        icon_path: "/icons/sqldeveloper-original.svg",
    },
    KeywordPattern {
        pattern: "MySQL",
        pattern_lower: "mysql",
        icon_path: "/icons/mysql-original.svg",
    },
    KeywordPattern {
        pattern: "Bash",
        pattern_lower: "bash",
        icon_path: "/icons/bash-original.svg",
    },
    KeywordPattern {
        pattern: "C++",
        pattern_lower: "c++",
        icon_path: "/icons/cplusplus-original.svg",
    },
    KeywordPattern {
        pattern: "C#",
        pattern_lower: "c#",
        icon_path: "/icons/csharp-original.svg",
    },
    KeywordPattern {
        pattern: "Qt",
        pattern_lower: "qt",
        icon_path: "/icons/qt-original.svg",
    },
    KeywordPattern {
        pattern: "XML",
        pattern_lower: "xml",
        icon_path: "/icons/xml-original.svg",
    },
];

pub fn tokenize(text: &str) -> Vec<Segment> {
    if text.is_empty() {
        return Vec::new();
    }

    let lower = text.to_ascii_lowercase();
    let mut occupied = vec![false; text.len()];
    let mut matches = Vec::new();

    for pattern in KEYWORD_PATTERNS {
        for (start, _) in lower.match_indices(pattern.pattern_lower) {
            let end = start + pattern.pattern_lower.len();

            if is_boundary(text, start, end)
                && !is_within_url(text, start, end)
                && !occupied[start..end].iter().any(|slot| *slot)
            {
                matches.push(MatchedRange {
                    start,
                    end,
                    icon_path: pattern.icon_path,
                });
                for idx in start..end {
                    occupied[idx] = true;
                }
            }
        }
    }

    matches.sort_by_key(|m| m.start);

    let mut segments = Vec::new();
    let mut cursor = 0;
    for m in matches {
        if cursor < m.start {
            let slice = &text[cursor..m.start];
            if !slice.is_empty() {
                segments.push(Segment::Text(slice.to_string()));
            }
        }
        let original = text[m.start..m.end].to_string();
        segments.push(Segment::Icon(IconMatch {
            token: original,
            icon_path: m.icon_path,
        }));
        cursor = m.end;
    }
    if cursor < text.len() {
        let slice = &text[cursor..];
        if !slice.is_empty() {
            segments.push(Segment::Text(slice.to_string()));
        }
    }

    segments
}

struct MatchedRange {
    start: usize,
    end: usize,
    icon_path: &'static str,
}

fn is_boundary(text: &str, start: usize, end: usize) -> bool {
    is_start_boundary(text, start) && is_end_boundary(text, end)
}

fn is_within_url(text: &str, start: usize, end: usize) -> bool {
    let (segment_start, segment_end) = surrounding_segment(text, start, end);
    if segment_start >= segment_end {
        return false;
    }
    let raw_segment = &text[segment_start..segment_end];
    let segment = raw_segment
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '(' | ')' | '[' | ']' | '<' | '>'));
    if segment.is_empty() {
        return false;
    }
    let segment =
        segment.trim_end_matches(|ch: char| matches!(ch, '.' | ',' | ';' | ':' | '!' | '?'));
    if segment.is_empty() {
        return false;
    }

    let lower = segment.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
        || lower.starts_with("www.")
        || lower.contains("://")
    {
        return true;
    }

    if looks_like_domain(&segment) {
        return true;
    }

    false
}

fn surrounding_segment(text: &str, start: usize, end: usize) -> (usize, usize) {
    let segment_start = locate_segment_start(text, start);
    let segment_end = locate_segment_end(text, end);
    (segment_start, segment_end)
}

fn locate_segment_start(text: &str, mut index: usize) -> usize {
    while index > 0 {
        if let Some((prev_index, ch)) = text[..index].char_indices().next_back() {
            if ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | '<' | '"' | '\'') {
                return prev_index + ch.len_utf8();
            }
            index = prev_index;
        } else {
            break;
        }
    }
    0
}

fn locate_segment_end(text: &str, mut index: usize) -> usize {
    let len = text.len();
    while index < len {
        if let Some((offset, ch)) = text[index..].char_indices().next() {
            if ch.is_whitespace() || matches!(ch, ')' | ']' | '}' | '>' | '"' | '\'') {
                return index + offset;
            }
            index += ch.len_utf8();
        } else {
            break;
        }
    }
    len
}

fn looks_like_domain(segment: &str) -> bool {
    if segment.chars().any(|ch| ch.is_whitespace()) {
        return false;
    }
    if !segment.contains('.') {
        return false;
    }

    let mut trimmed = segment;
    if let Some(eq_index) = trimmed.find("://") {
        trimmed = &trimmed[eq_index + 3..];
    }

    let mut parts = trimmed.splitn(2, '/');
    let host = parts.next().unwrap_or("");
    if host.is_empty() {
        return false;
    }

    if host.contains('@') {
        // Likely an email address or similar; treat as URL-like.
        return true;
    }

    let mut labels = host.split('.');
    let mut label_count = 0;
    while let Some(label) = labels.next() {
        if label.is_empty()
            || !label
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            return false;
        }
        label_count += 1;
    }

    if label_count < 2 {
        return false;
    }

    let tld = host
        .rsplit('.')
        .next()
        .unwrap_or("")
        .trim_matches(|ch: char| matches!(ch, '-' | '_'));
    if tld.len() < 2 || !tld.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return false;
    }

    true
}

fn is_start_boundary(text: &str, start: usize) -> bool {
    if start == 0 {
        return true;
    }
    text[..start]
        .chars()
        .rev()
        .next()
        .map(|ch| !is_keyword_char(ch))
        .unwrap_or(true)
}

fn is_end_boundary(text: &str, end: usize) -> bool {
    if end >= text.len() {
        return true;
    }
    text[end..]
        .chars()
        .next()
        .map(|ch| !is_keyword_char(ch))
        .unwrap_or(true)
}

fn is_keyword_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '+' | '#' | '/')
}

thread_local! {
    static ICON_SOURCES: RefCell<HashMap<&'static str, String>> = RefCell::new(HashMap::new());
    static PRELOAD_STARTED: RefCell<bool> = RefCell::new(false);
}

pub fn preload_all_icons() -> Result<(), JsValue> {
    PRELOAD_STARTED.with(|flag| {
        let mut started = flag.borrow_mut();
        if *started {
            return;
        }
        *started = true;
        spawn_local(async {
            if let Err(err) = preload_icons_async().await {
                utils::log(&format!("Failed to preload keyword icons: {:?}", err));
            }
        });
    });
    Ok(())
}

pub fn icon_source(icon_path: &str) -> String {
    ICON_SOURCES.with(|store| {
        store
            .borrow()
            .get(icon_path)
            .cloned()
            .unwrap_or_else(|| icon_path.to_string())
    })
}

async fn preload_icons_async() -> Result<(), JsValue> {
    let Some(window) = web_sys::window() else {
        return Ok(());
    };

    let priority = ["/favicon.ico", "/images/alexandre.webp"];
    for &asset in &priority {
        if asset == "/images/alexandre.webp" {
            if ICON_SOURCES.with(|store| store.borrow().contains_key(asset)) {
                continue;
            }
            if let Ok(url) = fetch_icon_url(&window, asset).await {
                ICON_SOURCES.with(|store| {
                    store.borrow_mut().insert(asset, url);
                });
            }
        } else {
            let _ = fetch_resource(&window, asset).await;
        }
    }

    let mut pending = Vec::new();
    ICON_SOURCES.with(|store| {
        let store = store.borrow();
        for icon_path in KEYWORD_PATTERNS.iter().map(|pattern| pattern.icon_path) {
            if !store.contains_key(icon_path) && icon_path != "/images/alexandre.webp" {
                pending.push(icon_path);
            }
        }
    });

    let tasks = pending.into_iter().map(|icon_path| {
        let window = window.clone();
        async move {
            let result = fetch_icon_url(&window, icon_path).await;
            (icon_path, result)
        }
    });

    let stream = stream::iter(tasks).buffer_unordered(ICON_PRELOAD_CONCURRENCY);
    pin_mut!(stream);
    while let Some((icon_path, result)) = stream.next().await {
        match result {
            Ok(url) => ICON_SOURCES.with(|store| {
                store.borrow_mut().insert(icon_path, url);
            }),
            Err(err) => utils::log(&format!("Failed to cache icon {icon_path}: {:?}", err)),
        }
    }

    Ok(())
}

async fn fetch_icon_url(
    window: &web_sys::Window,
    icon_path: &'static str,
) -> Result<String, JsValue> {
    let opts = RequestInit::new();
    opts.set_method("GET");
    opts.set_mode(RequestMode::SameOrigin);
    let request = Request::new_with_str_and_init(icon_path, &opts)?;
    let response_value = JsFuture::from(window.fetch_with_request(&request)).await?;
    let response: Response = response_value.dyn_into()?;
    if !response.ok() {
        return Err(JsValue::from_str("icon fetch response not ok"));
    }
    let blob_promise = response.blob()?;
    let blob_value = JsFuture::from(blob_promise).await?;
    let blob: Blob = blob_value.dyn_into()?;
    Url::create_object_url_with_blob(&blob)
}

async fn fetch_resource(window: &web_sys::Window, path: &str) -> Result<(), JsValue> {
    let opts = RequestInit::new();
    opts.set_method("GET");
    opts.set_mode(RequestMode::SameOrigin);
    let request = Request::new_with_str_and_init(path, &opts)?;
    let response_value = JsFuture::from(window.fetch_with_request(&request)).await?;
    let response: Response = response_value.dyn_into()?;
    if response.ok() {
        if let Ok(buffer_promise) = response.array_buffer() {
            let _ = JsFuture::from(buffer_promise).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_marks_multiword_keywords() {
        let segments = tokenize("Working with GitHub Actions and Rust.");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Working with ".to_string()),
                Segment::Icon(IconMatch {
                    token: "GitHub Actions".to_string(),
                    icon_path: "/icons/githubactions-original.svg"
                }),
                Segment::Text(" and ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Rust".to_string(),
                    icon_path: "/icons/rust-original.svg"
                }),
                Segment::Text(".".to_string())
            ]
        );
    }

    #[test]
    fn tokenize_respects_word_boundaries() {
        let segments = tokenize("Goal oriented Go developer");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Goal oriented ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Go".to_string(),
                    icon_path: "/icons/go-original.svg"
                }),
                Segment::Text(" developer".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_handles_punctuation() {
        let segments = tokenize("Rust, Python; AWS.");
        assert_eq!(
            segments,
            vec![
                Segment::Icon(IconMatch {
                    token: "Rust".to_string(),
                    icon_path: "/icons/rust-original.svg"
                }),
                Segment::Text(", ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Python".to_string(),
                    icon_path: "/icons/python-original.svg"
                }),
                Segment::Text("; ".to_string()),
                Segment::Icon(IconMatch {
                    token: "AWS".to_string(),
                    icon_path:
                        "/icons/amazonwebservices-original-wordmark.svg"
                }),
                Segment::Text(".".to_string())
            ]
        );
    }

    #[test]
    fn tokenize_prefers_longest_match() {
        let segments = tokenize("Shipping builds in Unreal Engine 5 pipelines.");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Shipping builds in ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Unreal Engine 5".to_string(),
                    icon_path: "/icons/unrealengine-original.svg"
                }),
                Segment::Text(" pipelines.".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_detects_figma_and_jira() {
        let segments = tokenize("Tooling: Figma, Jira.");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Tooling: ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Figma".to_string(),
                    icon_path: "/icons/figma-original.svg"
                }),
                Segment::Text(", ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Jira".to_string(),
                    icon_path: "/icons/jira-original.svg"
                }),
                Segment::Text(".".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_detects_sql_and_gcp() {
        let segments = tokenize("Data layer runs on SQL with GCP services.");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Data layer runs on ".to_string()),
                Segment::Icon(IconMatch {
                    token: "SQL".to_string(),
                    icon_path: "/icons/sqldeveloper-original.svg"
                }),
                Segment::Text(" with ".to_string()),
                Segment::Icon(IconMatch {
                    token: "GCP".to_string(),
                    icon_path: "/icons/googlecloud-original.svg"
                }),
                Segment::Text(" services.".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_detects_android() {
        let segments = tokenize("Android development with Jetpack.");
        assert_eq!(
            segments,
            vec![
                Segment::Icon(IconMatch {
                    token: "Android".to_string(),
                    icon_path: "/icons/android-original.svg"
                }),
                Segment::Text(" development with Jetpack.".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_detects_mysql() {
        let segments = tokenize("MySQL backups and SQL migrations.");
        assert_eq!(
            segments,
            vec![
                Segment::Icon(IconMatch {
                    token: "MySQL".to_string(),
                    icon_path: "/icons/mysql-original.svg"
                }),
                Segment::Text(" backups and ".to_string()),
                Segment::Icon(IconMatch {
                    token: "SQL".to_string(),
                    icon_path: "/icons/sqldeveloper-original.svg"
                }),
                Segment::Text(" migrations.".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_detects_jupyter_linear_maya_qt() {
        let segments = tokenize("Stack: Jupyter Notebook, Linear, Maya, Qt.");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Stack: ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Jupyter Notebook".to_string(),
                    icon_path: "/icons/jupyter-original-wordmark.svg"
                }),
                Segment::Text(", ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Linear".to_string(),
                    icon_path: "/icons/linear-original.svg"
                }),
                Segment::Text(", ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Maya".to_string(),
                    icon_path: "/icons/maya-original.svg"
                }),
                Segment::Text(", ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Qt".to_string(),
                    icon_path: "/icons/qt-original.svg"
                }),
                Segment::Text(".".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_leaves_plain_google_text() {
        let segments = tokenize("Google search mastery.");
        assert_eq!(
            segments,
            vec![Segment::Text("Google search mastery.".to_string())]
        );
    }

    #[test]
    fn tokenize_detects_linkedin() {
        let segments = tokenize("Connect on LinkedIn today.");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Connect on ".to_string()),
                Segment::Icon(IconMatch {
                    token: "LinkedIn".to_string(),
                    icon_path: "/icons/linkedin-original.svg"
                }),
                Segment::Text(" today.".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_detects_meta() {
        let segments = tokenize("Shipped experiences with Meta Platforms and Meta teams.");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Shipped experiences with ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Meta Platforms".to_string(),
                    icon_path: "/icons/meta-original.svg"
                }),
                Segment::Text(" and ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Meta".to_string(),
                    icon_path: "/icons/meta-original.svg"
                }),
                Segment::Text(" teams.".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_detects_alexandre() {
        let segments = tokenize("Meet Alexandre DO-O ALMEIDA, better known as Alexandre.");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Meet ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Alexandre DO-O ALMEIDA".to_string(),
                    icon_path: "/images/alexandre.webp"
                }),
                Segment::Text(", better known as Alexandre.".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_detects_profile_load_line() {
        let segments = tokenize("Profile loaded for Alexandre DO-O ALMEIDA.");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Profile loaded for ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Alexandre DO-O ALMEIDA".to_string(),
                    icon_path: "/images/alexandre.webp"
                }),
                Segment::Text(".".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_detects_languages_in_bullet_list() {
        let segments = tokenize("- Rust\n- Python\n- TypeScript");
        let icons: Vec<String> = segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Icon(icon) => Some(icon.token.to_ascii_lowercase()),
                _ => None,
            })
            .collect();
        for language in ["rust", "python", "typescript"] {
            assert!(
                icons.contains(&language.to_string()),
                "Expected icon for `{language}` in bullet list, found icons: {icons:?}"
            );
        }
    }

    #[test]
    fn tokenize_detects_language_in_code_fence() {
        let segments = tokenize("```rust\nfn main() {}\n```");
        let icons: Vec<String> = segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Icon(icon) => Some(icon.token.to_ascii_lowercase()),
                _ => None,
            })
            .collect();
        assert!(
            icons.contains(&"rust".to_string()),
            "Expected icon for `rust` in code fence, found icons: {icons:?}"
        );
    }

    #[test]
    fn tokenize_ignores_keywords_inside_urls() {
        let text = "Visit https://www.linkedin.com/in/aleqs for details.";
        let segments = tokenize(text);
        assert_eq!(segments, vec![Segment::Text(text.to_string())]);
    }

    #[test]
    fn tokenize_ignores_keywords_inside_domain_only_urls() {
        let text = "Docs live at google.com/cloud.";
        let segments = tokenize(text);
        assert_eq!(segments, vec![Segment::Text(text.to_string())]);
    }
}
