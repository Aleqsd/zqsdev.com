use crate::state::{AppState, Education, Experience, Profile, Project, TerminalData};
use crate::utils;
use std::collections::BTreeMap;

pub struct CommandDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
}

const AI_MODEL_NAME: &str = "llama-3.1-8b-instant";

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
}

#[derive(Debug)]
pub enum CommandError {
    NotFound { command: String },
    Message(String),
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
        "ai" => execute_ai(state),
        "clear" => Ok(CommandAction::Clear),
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
    if let Some(location) = &profile.location {
        facts.push(format!("Location: {location}"));
    }
    if let Some(email) = &profile.email {
        facts.push(format!("Email: {email}"));
    }
    if let Some(languages) = &profile.languages {
        if !languages.is_empty() {
            let joined = languages
                .iter()
                .map(|lang| lang.to_uppercase())
                .collect::<Vec<_>>()
                .join(", ");
            facts.push(format!("Languages: {joined}"));
        }
    }
    if !facts.is_empty() {
        lines.push(String::new());
        lines.push("Quick facts:".to_string());
        for fact in facts {
            lines.push(format!("  • {fact}"));
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
    Ok(CommandAction::Output(format_projects(&data.projects)))
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
    let target = data
        .profile
        .links
        .resume_url
        .clone()
        .unwrap_or_else(|| "https://cv.zqsdev.com".to_string());
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

fn render_help() -> String {
    let mut lines = Vec::new();
    lines.push("Available commands:".to_string());
    for cmd in COMMAND_DEFINITIONS {
        lines.push(format!("  {:10} — {}", cmd.name, cmd.description));
    }
    lines.push(String::new());
    lines.push(
        "Tip: Toggle the AI Mode button to ask the assistant questions about Alexandre."
            .to_string(),
    );
    lines.join("\n")
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
            languages: Some(vec!["English".to_string(), "Français".to_string()]),
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
            Vec::<Project>::new(),
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
            output.contains("ENGLISH, FRANÇAIS"),
            "Languages should appear uppercased in contact output:\n{output}"
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
            html.contains("https://cv.zqsdev.com"),
            "Résumé link should surface the configured URL: {html}"
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

fn format_projects(projects: &[Project]) -> String {
    let mut lines = Vec::new();
    for project in projects {
        lines.push(project.name.clone());
        lines.push(format!("  {}", project.desc));
        if !project.tech.is_empty() {
            lines.push(format!("  Tech: {}", project.tech.join(", ")));
        }
        if let Some(link) = &project.link {
            lines.push(format!("  Link: {link}"));
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
        let languages_text = languages
            .iter()
            .map(|lang| utils::escape_html(&lang.to_uppercase()))
            .collect::<Vec<_>>()
            .join(", ");
        html.push_str(&format!(
            "<div class=\"contact-meta\"><span class=\"contact-label\">Languages</span><span class=\"contact-value\">{}</span></div>",
            languages_text
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
        items.push(render_link_item("📄 Résumé", resume_url, false));
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
