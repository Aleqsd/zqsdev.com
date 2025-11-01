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

const KEYWORD_PATTERNS: &[KeywordPattern] = &[
    KeywordPattern {
        pattern: "Amazon Web Services",
        pattern_lower: "amazon web services",
        icon_path: "/icons/devicons/amazonwebservices/amazonwebservices-original-wordmark.svg",
    },
    KeywordPattern {
        pattern: "Google Cloud Platform",
        pattern_lower: "google cloud platform",
        icon_path: "/icons/devicons/googlecloud/googlecloud-original.svg",
    },
    KeywordPattern {
        pattern: "GitHub Actions",
        pattern_lower: "github actions",
        icon_path: "/icons/devicons/githubactions/githubactions-original.svg",
    },
    KeywordPattern {
        pattern: "Visual Studio",
        pattern_lower: "visual studio",
        icon_path: "/icons/devicons/visualstudio/visualstudio-original.svg",
    },
    KeywordPattern {
        pattern: "Google Cloud",
        pattern_lower: "google cloud",
        icon_path: "/icons/devicons/googlecloud/googlecloud-original.svg",
    },
    KeywordPattern {
        pattern: "AWS Lambda",
        pattern_lower: "aws lambda",
        icon_path: "/icons/devicons/amazonwebservices/amazonwebservices-original-wordmark.svg",
    },
    KeywordPattern {
        pattern: "GitLab CI",
        pattern_lower: "gitlab ci",
        icon_path: "/icons/devicons/gitlab/gitlab-original.svg",
    },
    KeywordPattern {
        pattern: "Unreal Engine 5",
        pattern_lower: "unreal engine 5",
        icon_path: "/icons/devicons/unrealengine/unrealengine-original.svg",
    },
    KeywordPattern {
        pattern: "Slack API",
        pattern_lower: "slack api",
        icon_path: "/icons/devicons/slack/slack-original.svg",
    },
    KeywordPattern {
        pattern: "Unreal Engine",
        pattern_lower: "unreal engine",
        icon_path: "/icons/devicons/unrealengine/unrealengine-original.svg",
    },
    KeywordPattern {
        pattern: "Node.js",
        pattern_lower: "node.js",
        icon_path: "/icons/devicons/nodejs/nodejs-original.svg",
    },
    KeywordPattern {
        pattern: "NodeJS",
        pattern_lower: "nodejs",
        icon_path: "/icons/devicons/nodejs/nodejs-original.svg",
    },
    KeywordPattern {
        pattern: "TypeScript",
        pattern_lower: "typescript",
        icon_path: "/icons/devicons/typescript/typescript-original.svg",
    },
    KeywordPattern {
        pattern: "JavaScript",
        pattern_lower: "javascript",
        icon_path: "/icons/devicons/javascript/javascript-original.svg",
    },
    KeywordPattern {
        pattern: "Kubernetes",
        pattern_lower: "kubernetes",
        icon_path: "/icons/devicons/kubernetes/kubernetes-original.svg",
    },
    KeywordPattern {
        pattern: "Figma",
        pattern_lower: "figma",
        icon_path: "/icons/devicons/figma/figma-original.svg",
    },
    KeywordPattern {
        pattern: "Datadog",
        pattern_lower: "datadog",
        icon_path: "/icons/devicons/datadog/datadog-original.svg",
    },
    KeywordPattern {
        pattern: "Firebase",
        pattern_lower: "firebase",
        icon_path: "/icons/devicons/firebase/firebase-original.svg",
    },
    KeywordPattern {
        pattern: "Confluence",
        pattern_lower: "confluence",
        icon_path: "/icons/devicons/confluence/confluence-original.svg",
    },
    KeywordPattern {
        pattern: "Grafana",
        pattern_lower: "grafana",
        icon_path: "/icons/devicons/grafana/grafana-original.svg",
    },
    KeywordPattern {
        pattern: "Discord",
        pattern_lower: "discord",
        icon_path: "/icons/devicons/discordjs/discordjs-original.svg",
    },
    KeywordPattern {
        pattern: "Docker",
        pattern_lower: "docker",
        icon_path: "/icons/devicons/docker/docker-original.svg",
    },
    KeywordPattern {
        pattern: "GitHub",
        pattern_lower: "github",
        icon_path: "/icons/devicons/github/github-original.svg",
    },
    KeywordPattern {
        pattern: "Google",
        pattern_lower: "google",
        icon_path: "/icons/devicons/google/google-original.svg",
    },
    KeywordPattern {
        pattern: "Azure",
        pattern_lower: "azure",
        icon_path: "/icons/devicons/azure/azure-original.svg",
    },
    KeywordPattern {
        pattern: "Python",
        pattern_lower: "python",
        icon_path: "/icons/devicons/python/python-original.svg",
    },
    KeywordPattern {
        pattern: "GitLab",
        pattern_lower: "gitlab",
        icon_path: "/icons/devicons/gitlab/gitlab-original.svg",
    },
    KeywordPattern {
        pattern: "Jira",
        pattern_lower: "jira",
        icon_path: "/icons/devicons/jira/jira-original.svg",
    },
    KeywordPattern {
        pattern: "Unity",
        pattern_lower: "unity",
        icon_path: "/icons/devicons/unity/unity-original.svg",
    },
    KeywordPattern {
        pattern: "Unreal",
        pattern_lower: "unreal",
        icon_path: "/icons/devicons/unrealengine/unrealengine-original.svg",
    },
    KeywordPattern {
        pattern: "Slack",
        pattern_lower: "slack",
        icon_path: "/icons/devicons/slack/slack-original.svg",
    },
    KeywordPattern {
        pattern: "AWS",
        pattern_lower: "aws",
        icon_path: "/icons/devicons/amazonwebservices/amazonwebservices-original-wordmark.svg",
    },
    KeywordPattern {
        pattern: "GCP",
        pattern_lower: "gcp",
        icon_path: "/icons/devicons/googlecloud/googlecloud-original.svg",
    },
    KeywordPattern {
        pattern: "Rust",
        pattern_lower: "rust",
        icon_path: "/icons/devicons/rust/rust-original.svg",
    },
    KeywordPattern {
        pattern: "React",
        pattern_lower: "react",
        icon_path: "/icons/devicons/react/react-original.svg",
    },
    KeywordPattern {
        pattern: "Go",
        pattern_lower: "go",
        icon_path: "/icons/devicons/go/go-original.svg",
    },
    KeywordPattern {
        pattern: "Java",
        pattern_lower: "java",
        icon_path: "/icons/devicons/java/java-original.svg",
    },
    KeywordPattern {
        pattern: "Lua",
        pattern_lower: "lua",
        icon_path: "/icons/devicons/lua/lua-original.svg",
    },
    KeywordPattern {
        pattern: "SQL",
        pattern_lower: "sql",
        icon_path: "/icons/devicons/sqldeveloper/sqldeveloper-original.svg",
    },
    KeywordPattern {
        pattern: "Bash",
        pattern_lower: "bash",
        icon_path: "/icons/devicons/bash/bash-original.svg",
    },
    KeywordPattern {
        pattern: "C++",
        pattern_lower: "c++",
        icon_path: "/icons/devicons/cplusplus/cplusplus-original.svg",
    },
    KeywordPattern {
        pattern: "C#",
        pattern_lower: "c#",
        icon_path: "/icons/devicons/csharp/csharp-original.svg",
    },
    KeywordPattern {
        pattern: "C",
        pattern_lower: "c",
        icon_path: "/icons/devicons/c/c-original.svg",
    },
    KeywordPattern {
        pattern: "XML",
        pattern_lower: "xml",
        icon_path: "/icons/devicons/xml/xml-original.svg",
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

            if is_boundary(text, start, end) && !occupied[start..end].iter().any(|slot| *slot) {
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
    ch.is_ascii_alphanumeric() || matches!(ch, '+' | '#' | '.' | '/')
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
                    icon_path: "/icons/devicons/githubactions/githubactions-original.svg"
                }),
                Segment::Text(" and ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Rust".to_string(),
                    icon_path: "/icons/devicons/rust/rust-original.svg"
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
                    icon_path: "/icons/devicons/go/go-original.svg"
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
                    icon_path: "/icons/devicons/rust/rust-original.svg"
                }),
                Segment::Text(", ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Python".to_string(),
                    icon_path: "/icons/devicons/python/python-original.svg"
                }),
                Segment::Text("; ".to_string()),
                Segment::Icon(IconMatch {
                    token: "AWS".to_string(),
                    icon_path:
                        "/icons/devicons/amazonwebservices/amazonwebservices-original-wordmark.svg"
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
                    icon_path: "/icons/devicons/unrealengine/unrealengine-original.svg"
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
                    icon_path: "/icons/devicons/figma/figma-original.svg"
                }),
                Segment::Text(", ".to_string()),
                Segment::Icon(IconMatch {
                    token: "Jira".to_string(),
                    icon_path: "/icons/devicons/jira/jira-original.svg"
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
                    icon_path: "/icons/devicons/sqldeveloper/sqldeveloper-original.svg"
                }),
                Segment::Text(" with ".to_string()),
                Segment::Icon(IconMatch {
                    token: "GCP".to_string(),
                    icon_path: "/icons/devicons/googlecloud/googlecloud-original.svg"
                }),
                Segment::Text(" services.".to_string()),
            ]
        );
    }
}
