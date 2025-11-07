use crate::build_info;
use crate::state::{
    AppState, Award, Education, Experience, Profile, ProjectsCollection, TerminalData,
};
use crate::utils;
use js_sys::Math;
use std::collections::BTreeMap;

pub struct CommandDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
}

const AI_MODEL_NAME: &str = "llama-3.1-8b-instant";
const REPO_URL: &str = "https://github.com/Aleqsd/zqsdev.com";

pub const COMMAND_DEFINITIONS: &[CommandDefinition] = &[
    CommandDefinition {
        name: "help",
        description: "Show all available commands.",
        icon: "ℹ️",
    },
    CommandDefinition {
        name: "about",
        description: "Summarise the profile at a glance.",
        icon: "👤",
    },
    CommandDefinition {
        name: "skills",
        description: "Show skills grouped by category.",
        icon: "🛠️",
    },
    CommandDefinition {
        name: "experience",
        description: "List professional experiences.",
        icon: "💼",
    },
    CommandDefinition {
        name: "education",
        description: "Show education background.",
        icon: "🎓",
    },
    CommandDefinition {
        name: "projects",
        description: "List main projects.",
        icon: "🗂️",
    },
    CommandDefinition {
        name: "testimonials",
        description: "Hear from collaborators and leads.",
        icon: "💬",
    },
    CommandDefinition {
        name: "contact",
        description: "Show contact information and links.",
        icon: "✉️",
    },
    CommandDefinition {
        name: "resume",
        description: "Open the résumé in a new tab.",
        icon: "📄",
    },
    CommandDefinition {
        name: "faq",
        description: "Answer common recruiter questions.",
        icon: "❓",
    },
    CommandDefinition {
        name: "ai",
        description: "Learn how to use the AI Mode experience.",
        icon: "🧠",
    },
    CommandDefinition {
        name: "shaw",
        description: "Summon Shaw for a celebratory cameo.",
        icon: "🎬",
    },
    CommandDefinition {
        name: "pokemon",
        description: "Throw a Poké Ball to try to catch Pikachu.",
        icon: "⚡️",
    },
    CommandDefinition {
        name: "cookie",
        description: "Summon a secret cookie clicker mini game.",
        icon: "🍪",
    },
    CommandDefinition {
        name: "clear",
        description: "Clear the terminal output.",
        icon: "🧹",
    },
];

#[derive(Debug)]
pub enum CommandAction {
    Output(String),
    OutputHtml(String),
    Clear,
    Download(String),
    ShawEffect,
    PokemonAttempt(PokemonAttemptOutcome),
    CookieClicker,
}

#[derive(Debug)]
pub enum CommandError {
    NotFound { command: String },
    Message(String),
}

#[derive(Debug)]
pub struct PokemonAttemptOutcome {
    pub current_chance: u8,
    pub success: bool,
    pub next_chance: u8,
}

pub fn command_names() -> Vec<&'static str> {
    COMMAND_DEFINITIONS.iter().map(|cmd| cmd.name).collect()
}

pub fn suggestions(prefix: &str) -> Vec<&'static str> {
    let lower = prefix.to_ascii_lowercase();
    COMMAND_DEFINITIONS
        .iter()
        .map(|cmd| cmd.name)
        .filter(|name| name.starts_with(&lower))
        .collect()
}

pub fn autocomplete(prefix: &str) -> Option<&'static str> {
    if prefix.is_empty() {
        return None;
    }
    let lower = prefix.to_ascii_lowercase();
    let mut matches = COMMAND_DEFINITIONS
        .iter()
        .map(|cmd| cmd.name)
        .filter(|name| name.starts_with(&lower));
    let first = matches.next()?;
    if matches.next().is_none() {
        Some(first)
    } else {
        None
    }
}

pub fn execute(
    command: &str,
    state: &AppState,
    _args: &[&str],
) -> Result<CommandAction, CommandError> {
    let normalized = command.trim().to_ascii_lowercase();
    let result = match normalized.as_str() {
        "help" => Ok(CommandAction::Output(render_help())),
        "about" => execute_about(state),
        "skills" => execute_skills(state),
        "experience" => execute_experience(state),
        "education" => execute_education(state),
        "projects" => execute_projects(state),
        "testimonials" => execute_testimonials(state),
        "contact" => execute_contact(state),
        "resume" => execute_resume(state),
        "faq" => execute_faq(state),
        "shaw" | "sha" => execute_shaw(),
        "pokemon" | "pokeball" => execute_pokemon(state),
        "cookie" => execute_cookie(),
        "ai" => execute_ai(state),
        "clear" => Ok(CommandAction::Clear),
        "version" | "ver" => execute_version(state),
        _ => {
            return Err(CommandError::NotFound {
                command: normalized,
            })
        }
    };
    result.map_err(CommandError::Message)
}

fn find_definition(name: &str) -> Option<&'static CommandDefinition> {
    COMMAND_DEFINITIONS
        .iter()
        .find(|cmd| cmd.name.eq_ignore_ascii_case(name))
}

pub fn helper_label(command: &str) -> String {
    let trimmed = command.trim();
    match find_definition(trimmed) {
        Some(def) if !def.icon.is_empty() => format!("{} {}", def.icon, def.name),
        _ => trimmed.to_string(),
    }
}

fn ensure_data(state: &AppState) -> Result<&TerminalData, String> {
    state
        .data
        .as_ref()
        .ok_or_else(|| "Hold on… still loading résumé data. Try again in a second.".to_string())
}

fn execute_about(state: &AppState) -> Result<CommandAction, String> {
    let data = ensure_data(state)?;
    let profile = &data.profile;

    let mut lines = Vec::new();
    lines.push(format!("{} — {}", profile.name, profile.headline));

    let mut facts = Vec::new();
    let languages = profile.languages.clone().unwrap_or_default();
    if let Some(location) = &profile.location {
        facts.push(format!("Location: {location}"));
    }
    if let Some(email) = &profile.email {
        facts.push(format!("Email: {email}"));
    }
    if !facts.is_empty() {
        lines.push(String::new());
        lines.push("Quick facts:".to_string());
        for fact in facts {
            lines.push(format!("  • {fact}"));
        }
    }

    if !languages.is_empty() {
        lines.push(String::new());
        lines.push("Languages:".to_string());
        for language in languages {
            lines.push(format!("  - {language}"));
        }
    }

    if let Some(summary_en) = &profile.summary_en {
        lines.push(String::new());
        lines.push("Focus:".to_string());
        lines.push(format!("  {summary_en}"));
    }

    if let Some(summary_fr) = &profile.summary_fr {
        lines.push(String::new());
        lines.push("Résumé (FR):".to_string());
        lines.push(format!("  {summary_fr}"));
    }

    Ok(CommandAction::Output(lines.join("\n")))
}

fn execute_skills(state: &AppState) -> Result<CommandAction, String> {
    let data = ensure_data(state)?;
    Ok(CommandAction::Output(format_skills(&data.skills)))
}

fn execute_experience(state: &AppState) -> Result<CommandAction, String> {
    let data = ensure_data(state)?;
    Ok(CommandAction::Output(format_experience(&data.experiences)))
}

fn execute_education(state: &AppState) -> Result<CommandAction, String> {
    let data = ensure_data(state)?;
    Ok(CommandAction::Output(format_education(&data.education)))
}

fn execute_projects(state: &AppState) -> Result<CommandAction, String> {
    let data = ensure_data(state)?;
    Ok(CommandAction::OutputHtml(render_projects_html(
        &data.projects,
    )))
}

fn execute_testimonials(state: &AppState) -> Result<CommandAction, String> {
    let data = ensure_data(state)?;
    if data.testimonials.is_empty() {
        return Ok(CommandAction::Output(
            "No testimonials available yet. Check back soon.".to_string(),
        ));
    }

    let mut lines = Vec::new();
    lines.push("Testimonials:".to_string());
    for testimonial in &data.testimonials {
        lines.push(format!("\"{}\"", testimonial.quote));
        let mut attribution = testimonial.author.clone();
        if let Some(role) = &testimonial.role {
            if !role.trim().is_empty() {
                attribution = format!("{attribution} ({role})");
            }
        }
        lines.push(format!("  — {attribution}"));
        if let Some(link) = &testimonial.link {
            if !link.trim().is_empty() {
                lines.push(format!("    {link}"));
            }
        }
        lines.push(String::new());
    }
    if let Some(last) = lines.last() {
        if last.is_empty() {
            lines.pop();
        }
    }

    Ok(CommandAction::Output(lines.join("\n")))
}

fn execute_contact(state: &AppState) -> Result<CommandAction, String> {
    let data = ensure_data(state)?;
    Ok(CommandAction::OutputHtml(render_contact_html(
        &data.profile,
    )))
}

fn execute_resume(state: &AppState) -> Result<CommandAction, String> {
    let data = ensure_data(state)?;
    let base = data
        .profile
        .links
        .resume_url
        .clone()
        .unwrap_or_else(|| "https://cv.zqsdev.com".to_string());
    let target = utils::tag_resume_source(&base);
    Ok(CommandAction::Download(target))
}

fn execute_faq(state: &AppState) -> Result<CommandAction, String> {
    let data = ensure_data(state)?;
    if data.faqs.is_empty() {
        return Ok(CommandAction::Output(
            "No FAQ entries published yet.".to_string(),
        ));
    }

    let mut lines = Vec::new();
    lines.push("FAQ:".to_string());
    for (index, item) in data.faqs.iter().enumerate() {
        lines.push(format!("{idx}. Q: {}", item.question, idx = index + 1));
        lines.push(format!("   A: {}", item.answer));
        lines.push(String::new());
    }
    if let Some(last) = lines.last() {
        if last.is_empty() {
            lines.pop();
        }
    }

    Ok(CommandAction::Output(lines.join("\n")))
}

fn execute_ai(state: &AppState) -> Result<CommandAction, String> {
    let mut lines = Vec::new();
    lines.push("🧠 AI Mode quick reference:".to_string());
    lines.push(
        "  • Toggle the AI Mode button above the terminal to activate the assistant.".to_string(),
    );
    lines.push("  • While active, type a natural-language question or use the helper chips (`help`, `quit`).".to_string());
    lines.push("  • The assistant only answers using Alexandre DO-O ALMEIDA's résumé data. If it can't find something, it will say so.".to_string());
    lines.push(format!(
        "  • Model in use: {AI_MODEL_NAME} (Groq primary with Gemini then OpenAI fallback)."
    ));
    lines.push(String::new());
    if state.ai_mode {
        lines.push("AI Mode is currently active. Ask your question or type `quit` to return to classic mode.".to_string());
    } else {
        lines.push(
            "AI Mode is currently deactivated. Tap the AI Mode button to switch it on.".to_string(),
        );
    }

    Ok(CommandAction::Output(lines.join("\n")))
}

fn execute_version(state: &AppState) -> Result<CommandAction, String> {
    let mut lines = Vec::new();
    lines.push("Deployment versions:".to_string());
    lines.push(format_version_line(
        "Frontend",
        build_info::FRONTEND_VERSION,
        build_info::frontend_commit(),
        None,
    ));

    if let Some(info) = state.backend_version() {
        let parity = if info.version == build_info::FRONTEND_VERSION {
            "in sync"
        } else {
            "version mismatch"
        };
        lines.push(format_version_line(
            "Backend",
            &info.version,
            &info.commit,
            Some(parity),
        ));
    } else {
        lines.push("  Backend: unavailable (version endpoint unreachable)".to_string());
    }

    Ok(CommandAction::Output(lines.join("\n")))
}

fn render_help() -> String {
    let mut lines = Vec::new();
    lines.push("Available commands:".to_string());
    let name_width = COMMAND_DEFINITIONS
        .iter()
        .map(|cmd| cmd.name.len())
        .max()
        .unwrap_or(0)
        + 2;
    for cmd in COMMAND_DEFINITIONS {
        lines.push(format!(
            "  {:width$} — {}",
            cmd.name,
            cmd.description,
            width = name_width
        ));
    }
    lines.push(String::new());
    lines.push(
        "Tip: Toggle the AI Mode button to ask the assistant questions about Alexandre."
            .to_string(),
    );
    lines.push(
        "Developed in Rust by Alexandre DO-O ALMEIDA (Open source: https://github.com/Aleqsd/zqsdev.com)"
            .to_string(),
    );
    lines.join("\n")
}

fn execute_shaw() -> Result<CommandAction, String> {
    Ok(CommandAction::ShawEffect)
}

fn execute_pokemon(state: &AppState) -> Result<CommandAction, String> {
    let chance = state.pokemon_capture_chance();
    let roll = (Math::random() * 100.0).floor() as u8;
    let success = roll < chance;
    let next_chance = if success {
        1
    } else {
        chance.saturating_add(1).min(100)
    };

    Ok(CommandAction::PokemonAttempt(PokemonAttemptOutcome {
        current_chance: chance,
        success,
        next_chance,
    }))
}

fn execute_cookie() -> Result<CommandAction, String> {
    Ok(CommandAction::CookieClicker)
}

fn format_version_line(label: &str, version: &str, commit: &str, parity: Option<&str>) -> String {
    let mut line = match commit_link(commit) {
        Some(link) => format!(
            "  {label}: v{version} (commit {commit}) – {link}",
            link = link
        ),
        None => format!("  {label}: v{version} (commit unknown)"),
    };
    if let Some(note) = parity {
        line.push_str(&format!(" ({note})"));
    }
    line
}

fn commit_link(commit: &str) -> Option<String> {
    let trimmed = commit.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(format!("{REPO_URL}/commit/{trimmed}"))
    }
}

fn format_skills(skills: &BTreeMap<String, Vec<String>>) -> String {
    let mut lines = Vec::new();
    for (category, items) in skills {
        lines.push(format!("{category}:"));
        if items.is_empty() {
            lines.push("  (no skills listed)".to_string());
        } else {
            lines.push(format!("  - {}", items.join(", ")));
        }
        lines.push(String::new());
    }
    if let Some(last) = lines.last() {
        if last.is_empty() {
            lines.pop();
        }
    }
    lines.join("\n")
}

fn format_experience(experiences: &[Experience]) -> String {
    let mut lines = Vec::new();
    for experience in experiences {
        lines.push(format!("{} — {}", experience.title, experience.company));
        if let (Some(start), Some(end)) = (&experience.start, &experience.end) {
            lines.push(format!("  Duration: {start} → {end}"));
        }
        if let Some(location) = &experience.location {
            lines.push(format!("  Location: {location}"));
        }
        for highlight in &experience.highlights {
            lines.push(format!("  • {highlight}"));
        }
        lines.push(String::new());
    }
    if let Some(last) = lines.last() {
        if last.is_empty() {
            lines.pop();
        }
    }
    lines.join("\n")
}

fn format_education(education: &[Education]) -> String {
    let mut lines = Vec::new();
    for entry in education {
        lines.push(entry.degree.clone());
        lines.push(format!("  {}", entry.school));
        if let Some(years) = &entry.years {
            lines.push(format!("  Years: {years}"));
        }
        if let Some(location) = &entry.location {
            lines.push(format!("  Location: {location}"));
        }
        lines.push(String::new());
    }
    if let Some(last) = lines.last() {
        if last.is_empty() {
            lines.pop();
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Project, Publication};
    use wasm_bindgen_test::wasm_bindgen_test;

    fn stub_state() -> AppState {
        use crate::state::{FaqEntry, Profile, ProfileLinks, Testimonial};

        let mut state = AppState::new();

        let profile = Profile {
            name: "Alex".to_string(),
            headline: "Rustacean".to_string(),
            summary_fr: Some("Résumé FR".to_string()),
            summary_en: Some("English summary".to_string()),
            location: Some("Earth".to_string()),
            email: Some("alex@example.com".to_string()),
            links: ProfileLinks {
                github: Some("https://github.com/example".to_string()),
                linkedin: None,
                website: None,
                resume_url: Some("https://cv.zqsdev.com".to_string()),
            },
            languages: Some(vec![
                "English (TOEIC 990/990) - Full professional proficiency".to_string(),
                "French - Native or bilingual proficiency".to_string(),
                "Spanish - Limited working proficiency".to_string(),
            ]),
        };

        let mut skills = BTreeMap::new();
        skills.insert("Backend".to_string(), vec!["Rust".to_string()]);

        let testimonials = vec![Testimonial {
            quote: "Alex keeps the build green.".to_string(),
            author: "Jamie".to_string(),
            role: Some("CTO".to_string()),
            link: Some("https://example.com/jamie".to_string()),
        }];

        let faqs = vec![FaqEntry {
            question: "Remote?".to_string(),
            answer: "Yes.".to_string(),
        }];

        let data = TerminalData::new(
            profile,
            skills,
            Vec::<Experience>::new(),
            Vec::<Education>::new(),
            ProjectsCollection::default(),
            testimonials,
            faqs,
        );

        state.set_data(data);
        state
    }

    #[wasm_bindgen_test]
    fn suggestions_are_case_insensitive() {
        let result = suggestions("Pr");
        assert_eq!(result, vec!["projects"]);
    }

    #[wasm_bindgen_test]
    fn autocomplete_requires_unique_match() {
        assert_eq!(autocomplete("sk"), Some("skills"));
        // Multiple commands start with 'c', so autocomplete should hold back.
        assert_eq!(autocomplete("c"), None);
    }

    #[wasm_bindgen_test]
    fn helper_label_uses_icon_when_available() {
        let label = helper_label("help");
        assert!(
            label.starts_with("ℹ️"),
            "Helper label should start with icon: {label}"
        );
        assert!(
            label.ends_with("help"),
            "Helper label should end with command name"
        );
    }

    #[wasm_bindgen_test]
    fn helper_label_falls_back_to_command_name() {
        let label = helper_label("unknown");
        assert_eq!(label, "unknown");
    }

    #[wasm_bindgen_test]
    fn about_command_includes_focus_section() {
        let state = stub_state();
        let output = match execute("about", &state, &[]) {
            Ok(CommandAction::Output(text)) => text,
            other => panic!("unexpected action for about: {other:?}"),
        };
        assert!(
            output.contains("Focus:"),
            "About output should include focus section:\n{output}"
        );
    }

    #[wasm_bindgen_test]
    fn testimonials_command_lists_entries() {
        let state = stub_state();
        let output = match execute("testimonials", &state, &[]) {
            Ok(CommandAction::Output(text)) => text,
            other => panic!("unexpected action for testimonials: {other:?}"),
        };
        assert!(
            output.contains("Alex keeps the build green."),
            "Testimonial quote missing:\n{output}"
        );
        assert!(
            output.contains("— Jamie (CTO)"),
            "Testimonial attribution missing:\n{output}"
        );
        assert!(
            output.contains("https://example.com/jamie"),
            "Testimonial link missing:\n{output}"
        );
    }

    #[wasm_bindgen_test]
    fn faq_command_numbers_questions() {
        let state = stub_state();
        let output = match execute("faq", &state, &[]) {
            Ok(CommandAction::Output(text)) => text,
            other => panic!("unexpected action for faq: {other:?}"),
        };
        assert!(
            output.contains("1. Q: Remote?"),
            "FAQ question missing numbering:\n{output}"
        );
        assert!(output.contains("A: Yes."), "FAQ answer missing:\n{output}");
    }

    #[wasm_bindgen_test]
    fn cookie_command_triggers_cookie_action() {
        let state = stub_state();
        let action = execute("cookie", &state, &[]).expect("cookie command should succeed");
        match action {
            CommandAction::CookieClicker => {}
            other => panic!("expected cookie clicker action, got {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn contact_command_includes_profile_details() {
        let state = stub_state();
        let action = execute("contact", &state, &[]).expect("command should succeed");

        let output = match action {
            CommandAction::OutputHtml(html) => html,
            other => panic!("expected html output, got {other:?}"),
        };

        assert!(
            output.contains("mailto:alex@example.com"),
            "Contact HTML should include mailto link:\n{output}"
        );
        assert!(
            output.contains("contact-links"),
            "Contact HTML should include links section markup:\n{output}"
        );
        assert!(
            output.contains("Résumé (FR)"),
            "French summary section missing from contact output:\n{output}"
        );
        assert!(
            output.contains("English summary"),
            "English summary missing from contact output:\n{output}"
        );
        assert!(
            output.contains("<li>English (TOEIC 990/990) - Full professional proficiency</li>"),
            "Languages should surface detailed proficiency in contact output with preserved casing:\n{output}"
        );
    }

    #[wasm_bindgen_test]
    fn contact_command_handles_missing_french_summary() {
        let mut state = stub_state();
        let mut data = state
            .data
            .clone()
            .expect("stub state should include résumé data");
        data.profile.summary_fr = None;
        data.profile.links.resume_url = None;
        state.set_data(data);

        let action = execute("contact", &state, &[]).expect("contact command should succeed");
        let CommandAction::OutputHtml(output) = action else {
            panic!("expected html output");
        };

        assert!(
            !output.contains("Résumé (FR)"),
            "Contact HTML should omit the French summary when unavailable:\n{output}"
        );
        assert!(
            !output.contains("cv.zqsdev.com"),
            "Contact HTML should hide resume link when not provided:\n{output}"
        );
        assert!(
            output.contains("ENGLISH (TOEIC 990/990) - FULL PROFESSIONAL PROFICIENCY"),
            "Detailed languages should remain visible when summaries are missing:\n{output}"
        );
    }

    #[wasm_bindgen_test]
    fn ai_command_guides_user() {
        let state = stub_state();
        let action = execute("ai", &state, &[]).expect("ai command should succeed");
        let CommandAction::Output(text) = action else {
            panic!("AI command should return output");
        };
        assert!(
            text.contains("Toggle the AI Mode button"),
            "Guidance should mention the AI Mode button: {text}"
        );
        assert!(
            text.contains("currently deactivated") || text.contains("currently active"),
            "Guidance should mention the current AI state: {text}"
        );
        assert!(
            text.contains("Groq primary with Gemini then OpenAI fallback"),
            "Guidance should mention updated backend order: {text}"
        );
    }

    #[test]
    fn help_command_columns_align() {
        let output = super::render_help();
        let mut widths = Vec::new();
        for line in output.lines().filter(|line| line.contains('—')) {
            if let Some(prefix) = line.split('—').next() {
                widths.push(prefix.chars().count());
            }
        }
        let Some(first) = widths.first() else {
            panic!("Help output should include command rows:\n{output}");
        };
        assert!(
            widths.iter().all(|width| width == first),
            "Expected help command names to align, got widths {widths:?}\n{output}"
        );
        assert!(
            output.contains("Open source: https://github.com/Aleqsd/zqsdev.com"),
            "Help output should mention open source link:\n{output}"
        );
    }

    #[test]
    fn unknown_command_returns_not_found() {
        let state = AppState::new();
        match execute("made-up-command", &state, &[]) {
            Err(CommandError::NotFound { command }) => {
                assert_eq!(command, "made-up-command");
            }
            other => panic!("unexpected result for unknown command: {other:?}"),
        }
    }

    #[test]
    fn render_projects_html_omits_link_when_absent() {
        let collection = ProjectsCollection {
            projects: vec![Project {
                title: "Demo".to_string(),
                date: Some("2024".to_string()),
                description: "No external link provided.".to_string(),
                tech: vec!["Rust".to_string(), "Testing".to_string()],
                link: None,
            }],
            publications: Vec::new(),
            awards: Vec::new(),
        };

        let output = super::render_projects_html(&collection);
        assert!(
            output.contains("Demo"),
            "Project name should be present:\n{output}"
        );
        assert!(
            output.contains("<small>2024</small>"),
            "Project year should render within a small tag:\n{output}"
        );
        assert!(
            output.contains("<strong>Tech:</strong> Rust, Testing"),
            "Tech stack should be listed:\n{output}"
        );
        assert!(
            !output.contains("<a "),
            "Formatter should omit link anchors when no link is provided:\n{output}"
        );
    }

    #[test]
    fn render_projects_html_includes_clickable_link() {
        let collection = ProjectsCollection {
            projects: vec![Project {
                title: "Linked Project".to_string(),
                date: None,
                description: "Has a URL attached.".to_string(),
                tech: vec!["Rust".to_string()],
                link: Some("https://example.com/demo".to_string()),
            }],
            publications: Vec::new(),
            awards: Vec::new(),
        };

        let output = super::render_projects_html(&collection);
        assert!(
            output.contains(r#"href="https://example.com/demo""#),
            "Expected anchor href in HTML output:\n{output}"
        );
        assert!(
            output.contains("target=\"_blank\""),
            "Link should open in a new tab:\n{output}"
        );
        assert!(
            output.contains("rel=\"noopener noreferrer\""),
            "Link should include rel safety attributes:\n{output}"
        );
    }

    #[test]
    fn render_projects_html_omits_tech_when_empty() {
        let collection = ProjectsCollection {
            projects: vec![Project {
                title: "No Tech Listed".to_string(),
                date: None,
                description: "An entry focusing on achievements without a tech stack.".to_string(),
                tech: Vec::new(),
                link: Some("https://example.com".to_string()),
            }],
            publications: Vec::new(),
            awards: Vec::new(),
        };

        let output = super::render_projects_html(&collection);
        assert!(
            output.contains("No Tech Listed"),
            "Project name should appear:\n{output}"
        );
        assert!(
            output.contains(r#"href="https://example.com""#),
            "Formatter should still render links when present:\n{output}"
        );
        assert!(
            !output.contains("<strong>Tech:</strong>"),
            "Formatter should omit tech line when list is empty:\n{output}"
        );
    }

    #[test]
    fn render_projects_html_includes_publications_section() {
        let collection = ProjectsCollection {
            projects: Vec::new(),
            publications: vec![Publication {
                title: "Whitepaper".to_string(),
                date: Some("2023".to_string()),
                description: "Explores advanced rendering techniques.".to_string(),
                tech: vec!["Rust".to_string(), "WebGPU".to_string()],
                link: Some("https://example.com/whitepaper".to_string()),
            }],
            awards: Vec::new(),
        };

        let output = super::render_projects_html(&collection);
        assert!(
            output.contains("<h2>Publications</h2>"),
            "Section heading should render for publications:\n{output}"
        );
        assert!(
            output.contains("Whitepaper"),
            "Publication title should be present:\n{output}"
        );
        assert!(
            output.contains("<small>2023</small>"),
            "Publication date should render within a small tag:\n{output}"
        );
        assert!(
            output.contains("Rust, WebGPU"),
            "Publication tech stack should appear:\n{output}"
        );
    }

    #[test]
    fn render_projects_html_displays_awards_metadata() {
        let collection = ProjectsCollection {
            projects: Vec::new(),
            publications: Vec::new(),
            awards: vec![Award {
                title: "Top Innovator".to_string(),
                issuer: Some("TechConf".to_string()),
                date: Some("2022".to_string()),
                description: Some("Recognised for breakthrough in AI tooling.".to_string()),
            }],
        };

        let output = super::render_projects_html(&collection);
        assert!(
            output.contains("<h2>Awards</h2>"),
            "Section heading should render for awards:\n{output}"
        );
        assert!(
            output.contains("Top Innovator"),
            "Award title should be listed:\n{output}"
        );
        assert!(
            output.contains("<strong>Issuer:</strong> TechConf"),
            "Issuer metadata should be highlighted:\n{output}"
        );
        assert!(
            output.contains("<small>2022</small>"),
            "Award date should render within a small tag:\n{output}"
        );
        assert!(
            output.contains("breakthrough in AI tooling"),
            "Award description should appear when provided:\n{output}"
        );
    }

    #[test]
    fn contact_html_escapes_special_characters() {
        let profile = crate::state::Profile {
            name: "<Alex>".to_string(),
            headline: "Dev & \"Lead\"".to_string(),
            summary_fr: None,
            summary_en: None,
            location: Some("Earth & Mars".to_string()),
            email: Some("alex@example.com".to_string()),
            links: crate::state::ProfileLinks {
                github: Some("https://example.com?q=1&v=2".to_string()),
                linkedin: None,
                website: None,
                resume_url: None,
            },
            languages: None,
        };

        let html = super::render_contact_html(&profile);
        assert!(
            html.contains("&lt;Alex&gt;"),
            "Name should be escaped in HTML: {html}"
        );
        assert!(
            html.contains("Dev &amp; &quot;Lead&quot;"),
            "Headline should escape ampersands and quotes: {html}"
        );
        assert!(
            html.contains("Earth &amp; Mars"),
            "Location should escape ampersands: {html}"
        );
        assert!(
            !html.contains("<Alex>"),
            "Raw brackets should not remain in contact HTML: {html}"
        );
    }

    #[test]
    fn links_html_includes_resume_link() {
        let links = crate::state::ProfileLinks {
            github: None,
            linkedin: Some("https://linkedin.com/in/example".to_string()),
            website: Some("https://zqsdev.com".to_string()),
            resume_url: Some("https://cv.zqsdev.com".to_string()),
        };

        let html = super::render_links_html(&links).expect("links should render");
        assert!(
            html.contains(&crate::utils::tag_resume_source("https://cv.zqsdev.com")),
            "Résumé link should surface the tagged URL: {html}"
        );
        assert!(
            html.contains("📄 Résumé"),
            "Résumé link label should include icon: {html}"
        );
        assert!(
            !html.contains("download"),
            "Résumé link should not set the download attribute: {html}"
        );
        assert!(
            html.contains("🔗 LinkedIn"),
            "LinkedIn label should appear in links HTML: {html}"
        );
    }
}

fn render_projects_html(collection: &ProjectsCollection) -> String {
    let has_projects = !collection.projects.is_empty();
    let has_publications = !collection.publications.is_empty();
    let has_awards = !collection.awards.is_empty();

    if !has_projects && !has_publications && !has_awards {
        return String::new();
    }

    let mut html = String::from("<div class=\"projects\">");
    if has_projects {
        html.push_str("<section class=\"projects-group\">");
        html.push_str("<h2>Projects</h2>");
        for project in &collection.projects {
            push_project_like(
                &mut html,
                "project",
                &project.title,
                project.date.as_deref(),
                &project.description,
                &project.tech,
                project.link.as_deref(),
            );
        }
        html.push_str("</section>");
    }

    if has_publications {
        html.push_str("<section class=\"projects-group\">");
        html.push_str("<h2>Publications</h2>");
        for publication in &collection.publications {
            push_project_like(
                &mut html,
                "publication",
                &publication.title,
                publication.date.as_deref(),
                &publication.description,
                &publication.tech,
                publication.link.as_deref(),
            );
        }
        html.push_str("</section>");
    }

    if has_awards {
        html.push_str("<section class=\"projects-group\">");
        html.push_str("<h2>Awards</h2>");
        for award in &collection.awards {
            push_award(&mut html, award);
        }
        html.push_str("</section>");
    }
    html.push_str("</div>");
    html
}

fn push_project_like(
    html: &mut String,
    class_name: &str,
    title: &str,
    date: Option<&str>,
    description: &str,
    tech: &[String],
    link: Option<&str>,
) {
    html.push_str("<article class=\"");
    html.push_str(class_name);
    html.push_str("\">");
    html.push_str("<h3>");
    html.push_str(&utils::escape_html(title));
    if let Some(date) = date.filter(|value| !value.trim().is_empty()) {
        html.push_str(" <small>");
        html.push_str(&utils::escape_html(date));
        html.push_str("</small>");
    }
    html.push_str("</h3>");

    html.push_str("<p>");
    html.push_str(&utils::escape_html(description));
    html.push_str("</p>");

    let tech = tech
        .iter()
        .filter_map(|item| {
            let trimmed = item.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(utils::escape_html(trimmed))
            }
        })
        .collect::<Vec<_>>();
    if !tech.is_empty() {
        html.push_str("<p><strong>Tech:</strong> ");
        html.push_str(&tech.join(", "));
        html.push_str("</p>");
    }

    if let Some(link) = link.filter(|value| !value.trim().is_empty()) {
        let safe_link = utils::escape_html(link);
        html.push_str("<p><a href=\"");
        html.push_str(&safe_link);
        html.push_str("\" target=\"_blank\" rel=\"noopener noreferrer\">");
        html.push_str(&safe_link);
        html.push_str("</a></p>");
    }

    html.push_str("</article>");
}

fn push_award(html: &mut String, award: &Award) {
    html.push_str("<article class=\"award\">");
    html.push_str("<h3>");
    html.push_str(&utils::escape_html(&award.title));
    if let Some(date) = award
        .date
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        html.push_str(" <small>");
        html.push_str(&utils::escape_html(date));
        html.push_str("</small>");
    }
    html.push_str("</h3>");

    if let Some(issuer) = award
        .issuer
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        html.push_str("<p><strong>Issuer:</strong> ");
        html.push_str(&utils::escape_html(issuer));
        html.push_str("</p>");
    }

    if let Some(description) = award
        .description
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        html.push_str("<p>");
        html.push_str(&utils::escape_html(description));
        html.push_str("</p>");
    }

    html.push_str("</article>");
}

fn render_contact_html(profile: &Profile) -> String {
    let mut html = String::from(r#"<div class="contact-block">"#);
    html.push_str(&format!(
        "<div class=\"contact-header\"><strong>{}</strong><br><span class=\"contact-headline\">{}</span></div>",
        utils::escape_html(&profile.name),
        utils::escape_html(&profile.headline),
    ));

    if let Some(location) = &profile.location {
        html.push_str(&format!(
            "<div class=\"contact-meta\"><span class=\"contact-label\">Location</span><span class=\"contact-value\">{}</span></div>",
            utils::escape_html(location)
        ));
    }
    if let Some(email) = &profile.email {
        let safe_email = utils::escape_html(email);
        html.push_str(&format!(
            "<div class=\"contact-meta\"><span class=\"contact-label\">Email</span><span class=\"contact-value\"><a href=\"mailto:{email}\">{email}</a></span></div>",
            email = safe_email
        ));
    }

    if let Some(summary_en) = &profile.summary_en {
        html.push_str(&format!(
            "<div class=\"contact-section\"><span class=\"contact-section-title\">Summary (EN)</span><p>{}</p></div>",
            utils::escape_html(summary_en)
        ));
    }
    if let Some(summary_fr) = &profile.summary_fr {
        html.push_str(&format!(
            "<div class=\"contact-section\"><span class=\"contact-section-title\">Résumé (FR)</span><p>{}</p></div>",
            utils::escape_html(summary_fr)
        ));
    }

    if let Some(languages) = profile.languages.as_ref().filter(|langs| !langs.is_empty()) {
        let highlight_languages =
            profile.summary_en.as_ref().is_none() || profile.summary_fr.as_ref().is_none();
        let languages_html = languages
            .iter()
            .map(|lang| {
                let display = if highlight_languages {
                    lang.to_uppercase()
                } else {
                    lang.clone()
                };
                format!("<li>{}</li>", utils::escape_html(&display))
            })
            .collect::<Vec<_>>()
            .join("");
        html.push_str(&format!(
            "<div class=\"contact-meta contact-languages\"><span class=\"contact-label\">Languages</span><ul class=\"contact-language-list\">{}</ul></div>",
            languages_html
        ));
    }

    if let Some(links_html) = render_links_html(&profile.links) {
        html.push_str(&links_html);
    }

    html.push_str("</div>");
    html
}

fn render_links_html(links: &crate::state::ProfileLinks) -> Option<String> {
    let mut items = Vec::new();
    if let Some(github) = links.github.as_deref().filter(|url| !url.is_empty()) {
        items.push(render_link_item("🐙 GitHub", github, false));
    }
    if let Some(linkedin) = links.linkedin.as_deref().filter(|url| !url.is_empty()) {
        items.push(render_link_item("🔗 LinkedIn", linkedin, false));
    }
    if let Some(website) = links.website.as_deref().filter(|url| !url.is_empty()) {
        items.push(render_link_item("🌐 Website", website, false));
    }
    if let Some(resume_url) = links.resume_url.as_deref().filter(|url| !url.is_empty()) {
        let tagged = utils::tag_resume_source(resume_url);
        items.push(render_link_item("📄 Résumé", &tagged, false));
    }

    if items.is_empty() {
        None
    } else {
        Some(format!(
            "<div class=\"contact-section contact-links-section\"><span class=\"contact-section-title\">Links</span><ul class=\"contact-links\">{}</ul></div>",
            items.join("")
        ))
    }
}

fn render_link_item(label: &str, url: &str, download: bool) -> String {
    let safe_label = utils::escape_html(label);
    let safe_url = utils::escape_html(url);
    let download_attr = if download { " download" } else { "" };
    format!(
        "<li><span class=\"contact-link-label\">{label}</span><a href=\"{href}\" target=\"_blank\" rel=\"noopener noreferrer\"{download}>{text}</a></li>",
        label = safe_label,
        href = safe_url,
        download = download_attr,
        text = safe_url
    )
}
