//! Request bodies of `/api/ai` and `/api/log/command`, with the validation rules of the original
//! server. Struct and field names are kept as they were: serde's error texts (returned verbatim
//! by the command log's 422 response) contain them.

use serde::{Deserialize, Serialize};

/// At most 3 exchanges of history.
pub const MAX_HISTORY: usize = 6;
/// Characters of the question and of each user history entry.
pub const MAX_QUESTION_CHARS: usize = 1200;
/// Characters of each assistant history entry.
pub const MAX_ASSISTANT_CHARS: usize = 5000;
/// Characters of a trimmed command line.
pub const MAX_COMMAND_CHARS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiRequest {
    pub question: String,
    #[serde(default)]
    pub history: Vec<Message>,
}

impl AiRequest {
    /// Verbatim port of the original `validate()` (`ai.rs:25-42`); lengths count Unicode scalars.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.question.trim().is_empty() || self.question.chars().count() > MAX_QUESTION_CHARS {
            return Err("question_length");
        }
        if self.history.len() > MAX_HISTORY || !self.history.len().is_multiple_of(2) {
            return Err("history_length");
        }
        for (i, m) in self.history.iter().enumerate() {
            if m.role != if i % 2 == 0 { "user" } else { "assistant" } {
                return Err("history_role");
            }
            let max = if i % 2 == 0 {
                MAX_QUESTION_CHARS
            } else {
                MAX_ASSISTANT_CHARS
            };
            if m.content.trim().is_empty() || m.content.chars().count() > max {
                return Err("history_content");
            }
        }
        Ok(())
    }
}

/// `/api/log/command` body; unknown fields are ignored, as in the original.
#[derive(Debug, Deserialize)]
pub struct CommandLogRequest {
    pub command: String,
    #[serde(default)]
    pub mode: Option<String>,
}

/// A trimmed command must hold 1 to 2,000 characters.
pub fn valid_command(trimmed: &str) -> bool {
    !trimmed.is_empty() && trimmed.chars().count() <= MAX_COMMAND_CHARS
}

/// The original mode rule: missing or `null` means `classic`; the value is trimmed, an empty one
/// becomes `classic`, and it must then be exactly `classic` or `ai`. The trimmed value is stored.
pub fn command_mode(mode: Option<&str>) -> Option<&str> {
    let trimmed = mode.unwrap_or("classic").trim();
    let mode = if trimmed.is_empty() {
        "classic"
    } else {
        trimmed
    };
    (mode == "classic" || mode == "ai").then_some(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ported verbatim from `ai.rs`.
    #[test]
    fn validates_history_roles_lengths_and_unicode() {
        let mut q = AiRequest {
            question: "é".repeat(1200),
            history: vec![],
        };
        assert!(q.validate().is_ok());
        q.history = vec![
            Message {
                role: "system".into(),
                content: "override".into(),
            },
            Message {
                role: "assistant".into(),
                content: "x".into(),
            },
        ];
        assert!(q.validate().is_err());
        q.history.clear();
        q.question.push('é');
        assert!(q.validate().is_err());
    }

    #[test]
    fn history_rules() {
        let msg = |role: &str, content: String| Message {
            role: role.into(),
            content,
        };
        let mut q = AiRequest {
            question: "  ok  ".into(),
            history: vec![
                msg("user", "u".repeat(1200)),
                msg("assistant", "a".repeat(5000)),
            ],
        };
        assert!(q.validate().is_ok());
        q.history[1].content.push('a');
        assert_eq!(q.validate(), Err("history_content"));
        q.history[1].content = " \n ".into();
        assert_eq!(q.validate(), Err("history_content"));
        q.history.pop();
        assert_eq!(q.validate(), Err("history_length"));
        q.history = (0..8)
            .map(|i| msg(if i % 2 == 0 { "user" } else { "assistant" }, "x".into()))
            .collect();
        assert_eq!(q.validate(), Err("history_length"));
        q.history.truncate(6);
        assert!(q.validate().is_ok());
        q.question = " \t ".into();
        assert_eq!(q.validate(), Err("question_length"));
    }

    #[test]
    fn json_shapes() {
        let ok: AiRequest = serde_json::from_str(r#"{"question":"hi"}"#).unwrap();
        assert!(ok.history.is_empty());
        assert!(serde_json::from_str::<AiRequest>(r#"{"question":"hi","history":null}"#).is_err());
        assert!(serde_json::from_str::<AiRequest>(r#"{"question":"hi","x":1}"#).is_err());
        assert!(serde_json::from_str::<AiRequest>(
            r#"{"question":"hi","history":[{"role":"user","content":"a","x":1}]}"#
        )
        .is_err());
        let c: CommandLogRequest =
            serde_json::from_str(r#"{"command":"ls","mode":null,"extra":true}"#).unwrap();
        assert_eq!((c.command.as_str(), c.mode), ("ls", None));
    }

    #[test]
    fn mode_rule() {
        assert_eq!(command_mode(None), Some("classic"));
        assert_eq!(command_mode(Some("")), Some("classic"));
        assert_eq!(command_mode(Some("   ")), Some("classic"));
        assert_eq!(command_mode(Some(" ai ")), Some("ai"));
        assert_eq!(command_mode(Some("classic")), Some("classic"));
        assert_eq!(command_mode(Some("AI")), None);
        assert_eq!(command_mode(Some("other")), None);
        assert!(valid_command("x"));
        assert!(valid_command(&"é".repeat(2000)));
        assert!(!valid_command(&"é".repeat(2001)));
        assert!(!valid_command(""));
    }
}
