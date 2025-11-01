use crate::ai;
use crate::commands::{self, CommandAction, CommandError};
use crate::renderer::Renderer;
use crate::state::AppState;
use crate::utils;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

pub type SharedState = Rc<RefCell<AppState>>;
pub type SharedRenderer = Rc<Renderer>;

pub struct Terminal {
    state: SharedState,
    renderer: SharedRenderer,
}

pub enum HistoryDirection {
    Older,
    Newer,
}

const WELCOME_TYPE_DELAY_MS: u32 = 18;
const AI_HELPER_SUGGESTIONS: &[(&str, &str)] = &[("help", "AI help"), ("quit", "Quit AI")];
const AI_STATUS_ACTIVE: &str = "AI Mode: Activated";
const AI_STATUS_DEACTIVATED: &str = "AI Mode: Deactivated";
const AI_STATUS_BUSY: &str = "AI Mode: Activated — Synthesizing…";
const AI_ACTIVATED_INFO: &str =
    "🤖 AI Mode activated. Ask anything about Alexandre DO-O ALMEIDA's profile.";
const AI_DEACTIVATED_INFO: &str = "📟 AI Mode deactivated. Classic terminal helpers restored.";
const AI_HELP_MESSAGE: &str = "🤖 AI Mode help:\nYou're chatting with an assistant that only uses Alexandre's résumé data.\nAsk a question or type `quit` to exit AI Mode.";
const AI_DATA_LOADING: &str = "AI knowledge base still loading. Please try again shortly.";

impl Terminal {
    pub fn new(state: SharedState, renderer: SharedRenderer) -> Self {
        Self { state, renderer }
    }

    pub fn initialize(&self) -> Result<(), JsValue> {
        let (prompt_label, input_buffer, ai_mode) = {
            let state = self.state.borrow();
            (
                state.prompt_label.clone(),
                state.input_buffer.clone(),
                state.ai_mode,
            )
        };

        self.renderer.set_prompt_label(&prompt_label);
        self.renderer.update_input(&input_buffer);
        self.refresh_suggestions();
        self.renderer.apply_ai_mode(ai_mode)?;
        self.renderer.focus_terminal();
        Ok(())
    }

    pub fn push_system_message(&self, message: &str) {
        let _ = self.renderer.append_info_line(message);
    }

    pub fn submit_command(&self) -> Result<(), JsValue> {
        let input = {
            let state = self.state.borrow();
            state.input_buffer.clone()
        };

        let display_line = input.clone();
        let mut state_mut = self.state.borrow_mut();
        let prompt_label = state_mut.prompt_label.clone();
        let trimmed = input.trim().to_string();
        state_mut.remember_command(&trimmed);
        state_mut.input_buffer.clear();
        drop(state_mut);

        self.refresh_input();
        self.refresh_suggestions();

        self.renderer.append_command(&prompt_label, &display_line)?;

        if trimmed.is_empty() {
            return Ok(());
        }

        if self.ai_mode_active() {
            return self.handle_ai_mode_submission(trimmed);
        }

        let args: Vec<&str> = trimmed.split_whitespace().collect();
        let command = args.first().cloned().unwrap_or_default();
        let extra = if args.is_empty() { &[][..] } else { &args[1..] };

        let action = {
            let state = self.state.borrow();
            commands::execute(command, &state, extra)
        };

        match action {
            Ok(CommandAction::Output(text)) => {
                self.renderer.append_output_text(&text)?;
            }
            Ok(CommandAction::OutputHtml(html)) => {
                self.renderer.append_output_html(&html)?;
            }
            Ok(CommandAction::Clear) => {
                self.renderer.clear_output();
            }
            Ok(CommandAction::Download(url)) => {
                utils::open_link(&url);
                let confirmation = format!("Opening résumé at {url}");
                self.renderer.append_info_line(&confirmation)?;
            }
            Err(CommandError::NotFound { command }) => {
                self.handle_unknown_command(&command)?;
            }
            Err(CommandError::Message(message)) => {
                self.renderer.append_output_text(&message)?;
            }
        }

        Ok(())
    }

    pub fn toggle_ai_mode(&self) -> Result<(), JsValue> {
        let next = !self.ai_mode_active();
        self.update_ai_mode(next, true)
    }

    pub fn activate_ai_mode(&self) -> Result<(), JsValue> {
        if self.ai_mode_active() {
            return Ok(());
        }
        self.update_ai_mode(true, true)
    }

    fn handle_unknown_command(&self, command: &str) -> Result<(), JsValue> {
        let message =
            format!("Command not found: `{command}`\nType `help` to list available commands.");
        self.renderer.append_output_text(&message)?;
        let html = r#"Need a hand? <button type="button" class="ai-mode-cta" data-action="activate-ai-mode">Ask the AI assistant</button>"#;
        self.renderer.append_info_html(html)?;
        Ok(())
    }

    pub fn clear_input(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.input_buffer.clear();
            state.history_index = None;
        }
        self.refresh_input();
        self.refresh_suggestions();
    }

    pub fn append_character(&self, value: &str) {
        self.append_text(value);
    }

    pub fn append_text(&self, value: &str) {
        if value.is_empty() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.input_buffer.push_str(value);
            state.history_index = None;
        }
        self.refresh_input();
        self.refresh_suggestions();
    }

    pub fn delete_last_character(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.input_buffer.pop();
            state.history_index = None;
        }
        self.refresh_input();
        self.refresh_suggestions();
    }

    pub fn navigate_history(&self, direction: HistoryDirection) {
        let new_buffer = {
            let mut state = self.state.borrow_mut();
            select_history_entry(&mut state, direction)
        };

        if let Some(buffer) = new_buffer {
            self.renderer.update_input(&buffer);
            self.refresh_suggestions();
        }
    }

    pub fn autocomplete(&self) {
        let suggestion = {
            let state = self.state.borrow();
            commands::autocomplete(&state.input_buffer).map(|value| value.to_string())
        };

        if let Some(text) = suggestion {
            {
                let mut state = self.state.borrow_mut();
                state.input_buffer = text;
            }
            self.refresh_input();
            self.refresh_suggestions();
        }
    }

    pub fn execute_suggestion(&self, command: &str) -> Result<(), JsValue> {
        {
            let mut state = self.state.borrow_mut();
            state.input_buffer = command.to_string();
            state.history_index = None;
        }
        self.refresh_input();
        self.submit_command()
    }

    pub fn on_data_ready(&self) -> Result<(), JsValue> {
        let (welcome, resume_link) = {
            let state = self.state.borrow();
            let welcome = build_welcome_message(&state);
            let link = state
                .data
                .as_ref()
                .and_then(|data| {
                    data.profile
                        .links
                        .resume_url
                        .clone()
                        .filter(|value| !value.trim().is_empty())
                })
                .unwrap_or_else(|| "https://cv.zqsdev.com".to_string());
            (welcome, link)
        };

        let renderer = Rc::clone(&self.renderer);
        spawn_local(async move {
            if let Err(err) = renderer
                .type_output_text(&welcome, WELCOME_TYPE_DELAY_MS)
                .await
            {
                utils::log(&format!("Failed to animate welcome message: {:?}", err));
                if let Err(err) = renderer.append_output_text(&welcome) {
                    utils::log(&format!(
                        "Failed to render welcome message fallback: {:?}",
                        err
                    ));
                }
            }

            let link_html = format!(
                r#"If you want, you can just <a href="{url}" target="_blank" rel="noopener noreferrer">open the résumé</a>."#,
                url = resume_link
            );
            if let Err(err) = renderer.append_info_html(&link_html) {
                utils::log(&format!("Failed to append résumé link: {:?}", err));
            }
            let ai_cta_html = r#"Prefer to talk with an AI? <button type="button" class="ai-mode-cta" data-action="activate-ai-mode">Ask the AI assistant</button>"#;
            if let Err(err) = renderer.append_info_html(ai_cta_html) {
                utils::log(&format!(
                    "Failed to append AI assistant call-to-action: {:?}",
                    err
                ));
            }
        });

        Ok(())
    }

    fn refresh_input(&self) {
        let buffer = { self.state.borrow().input_buffer.clone() };
        self.renderer.update_input(&buffer);
    }

    fn refresh_suggestions(&self) {
        render_current_suggestions(&self.state, &self.renderer);
    }

    fn handle_ai_mode_submission(&self, input: String) -> Result<(), JsValue> {
        let normalized = input.trim().to_ascii_lowercase();
        if normalized == "help" {
            self.renderer.append_output_text(AI_HELP_MESSAGE)?;
            return Ok(());
        }
        if normalized == "quit" {
            return self.update_ai_mode(false, true);
        }
        self.queue_ai_answer(input)
    }

    fn queue_ai_answer(&self, question: String) -> Result<(), JsValue> {
        let data_ready = { self.state.borrow().data.is_some() };
        if !data_ready {
            self.renderer.append_info_line(AI_DATA_LOADING)?;
            return Ok(());
        }

        self.renderer.set_ai_indicator_text(AI_STATUS_BUSY);
        if let Err(err) = self.renderer.set_ai_busy(true) {
            utils::log(&format!("Failed to flag AI busy state: {:?}", err));
        }
        if let Err(err) = self.renderer.show_ai_loader() {
            utils::log(&format!("Failed to render AI loader: {:?}", err));
        }

        let renderer = Rc::clone(&self.renderer);
        let shared_state = Rc::clone(&self.state);

        spawn_local(async move {
            let result = ai::ask_ai(&question).await;

            match result {
                Ok(payload) => {
                    if payload.ai_enabled {
                        renderer.set_ai_indicator_text(AI_STATUS_ACTIVE);
                        if let Err(err) = renderer.append_output_text(&payload.answer) {
                            utils::log(&format!("Failed to render AI answer: {:?}", err));
                        }
                    } else {
                        {
                            let mut state = shared_state.borrow_mut();
                            state.set_ai_mode(false);
                        }
                        if let Err(err) = renderer.apply_ai_mode(false) {
                            utils::log(&format!("Failed to revert AI mode visuals: {:?}", err));
                        }
                        renderer.set_ai_indicator_text(AI_STATUS_DEACTIVATED);
                        render_current_suggestions(&shared_state, &renderer);
                        let mut notice = payload.answer.clone();
                        if let Some(reason) = payload.reason.as_ref() {
                            notice.push_str(&format!(" (limit: {reason})"));
                        }
                        if let Err(err) = renderer.append_info_line(&notice) {
                            utils::log(&format!("Failed to render AI limit info: {:?}", err));
                        }
                    }
                }
                Err(error) => {
                    let message = format!("AI error: {error}");
                    if let Err(err) = renderer.append_output_text(&message) {
                        utils::log(&format!("Failed to render AI error: {:?}", err));
                    }
                }
            }

            if let Err(err) = renderer.set_ai_busy(false) {
                utils::log(&format!("Failed to reset AI busy state: {:?}", err));
            }
            if let Err(err) = renderer.hide_ai_loader() {
                utils::log(&format!("Failed to remove AI loader: {:?}", err));
            }

            let status = if shared_state.borrow().ai_mode {
                AI_STATUS_ACTIVE
            } else {
                AI_STATUS_DEACTIVATED
            };
            renderer.set_ai_indicator_text(status);
        });

        Ok(())
    }

    fn update_ai_mode(&self, active: bool, announce: bool) -> Result<(), JsValue> {
        let previous = {
            let mut state = self.state.borrow_mut();
            let prev = state.ai_mode;
            state.set_ai_mode(active);
            prev
        };

        self.renderer.apply_ai_mode(active)?;
        self.renderer.set_ai_indicator_text(if active {
            AI_STATUS_ACTIVE
        } else {
            AI_STATUS_DEACTIVATED
        });
        if let Err(err) = self.renderer.set_ai_busy(false) {
            utils::log(&format!("Failed to reset AI busy flag: {:?}", err));
        }

        if announce && previous != active {
            let message = if active {
                AI_ACTIVATED_INFO
            } else {
                AI_DEACTIVATED_INFO
            };
            self.renderer.append_info_line(message)?;
        }

        if previous != active {
            self.refresh_suggestions();
        }

        Ok(())
    }

    fn ai_mode_active(&self) -> bool {
        self.state.borrow().ai_mode
    }
}

fn select_history_entry(state: &mut AppState, direction: HistoryDirection) -> Option<String> {
    if state.command_history.is_empty() {
        return None;
    }

    let len = state.command_history.len();

    let new_index = match (state.history_index, direction) {
        (None, HistoryDirection::Older) => Some(len - 1),
        (None, HistoryDirection::Newer) => None,
        (Some(0), HistoryDirection::Older) => Some(0),
        (Some(idx), HistoryDirection::Older) => Some(idx.saturating_sub(1)),
        (Some(idx), HistoryDirection::Newer) => {
            if idx + 1 >= len {
                None
            } else {
                Some(idx + 1)
            }
        }
    };

    state.history_index = new_index;

    let buffer = match new_index {
        Some(idx) => state.command_history[idx].clone(),
        None => String::new(),
    };

    state.input_buffer = buffer.clone();
    Some(buffer)
}

fn default_suggestions() -> Vec<&'static str> {
    let mut names = commands::command_names();
    if let Some(index) = names.iter().position(|name| *name == "resume") {
        let resume = names.remove(index);
        names.insert(0, resume);
    }
    names
}

fn render_current_suggestions(state: &SharedState, renderer: &SharedRenderer) {
    let (buffer, ai_mode) = {
        let state = state.borrow();
        (state.input_buffer.clone(), state.ai_mode)
    };
    let trimmed = buffer.trim().to_ascii_lowercase();

    let suggestions: Vec<(String, String)> = if ai_mode {
        AI_HELPER_SUGGESTIONS
            .iter()
            .filter(|(command, _)| trimmed.is_empty() || command.starts_with(&trimmed))
            .map(|(command, label)| ((*command).to_string(), (*label).to_string()))
            .collect()
    } else {
        let names: Vec<String> = if trimmed.is_empty() {
            default_suggestions()
                .into_iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            commands::suggestions(&buffer)
                .into_iter()
                .map(|s| s.to_string())
                .collect()
        };

        names
            .into_iter()
            .map(|command| {
                let label = commands::helper_label(&command);
                (command, label)
            })
            .collect()
    };

    renderer.render_suggestions(suggestions);
}

fn build_welcome_message(state: &AppState) -> String {
    if let Some(data) = &state.data {
        let mut lines = Vec::new();
        lines.push("Welcome to the ZQSDev interactive terminal!".to_string());
        lines.push(format!("Profile loaded for {}.", data.profile.name));
        lines.push(String::new());
        lines.push("Type `help` to view all available commands.".to_string());
        lines.push("Use the quick actions below to jump to key sections instantly.".to_string());
        lines.join("\n")
    } else {
        "Welcome! Loading résumé data…".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        Education, Experience, FaqEntry, Profile, ProfileLinks, Project, TerminalData, Testimonial,
    };
    use wasm_bindgen_test::wasm_bindgen_test;

    fn make_state_with_data() -> AppState {
        use std::collections::BTreeMap;

        let mut state = AppState::new();
        let profile = Profile {
            name: "Alex".to_string(),
            headline: "Rustacean".to_string(),
            summary_fr: None,
            summary_en: None,
            location: None,
            email: None,
            links: ProfileLinks {
                github: None,
                linkedin: None,
                website: None,
                resume_url: Some("https://cv.zqsdev.com".to_string()),
            },
            languages: None,
        };

        let data = TerminalData::new(
            profile,
            BTreeMap::new(),
            Vec::<Experience>::new(),
            Vec::<Education>::new(),
            Vec::<Project>::new(),
            Vec::<Testimonial>::new(),
            Vec::<FaqEntry>::new(),
        );
        state.set_data(data);
        state
    }

    #[wasm_bindgen_test]
    fn welcome_message_with_data_mentions_profile() {
        let state = make_state_with_data();
        let message = build_welcome_message(&state);
        assert!(message.contains("Profile loaded for Alex."));
        assert!(
            message.contains("Type `help`"),
            "Help hint missing:\n{message}"
        );
        assert!(
            message.contains("Use the quick actions below"),
            "Quick action hint missing:\n{message}"
        );
    }

    #[wasm_bindgen_test]
    fn welcome_message_without_data_is_loading() {
        let state = AppState::new();
        let message = build_welcome_message(&state);
        assert_eq!(message, "Welcome! Loading résumé data…");
    }

    #[wasm_bindgen_test]
    fn default_suggestions_execute_without_errors() {
        let state = make_state_with_data();
        let mut expected = crate::commands::command_names();
        if let Some(index) = expected.iter().position(|name| *name == "resume") {
            let resume = expected.remove(index);
            expected.insert(0, resume);
        }
        assert_eq!(
            super::default_suggestions(),
            expected,
            "Default suggestions should list every command with résumé primed first"
        );
        for command in super::default_suggestions() {
            let result = crate::commands::execute(command, &state, &[]);
            assert!(
                result.is_ok(),
                "Suggestion `{command}` should execute without errors"
            );
        }
    }

    #[wasm_bindgen_test]
    fn resume_helper_chip_is_prioritized() {
        let suggestions = super::default_suggestions();
        assert_eq!(
            suggestions.first().copied(),
            Some("resume"),
            "Résumé helper chip should be the first suggestion"
        );
    }

    #[wasm_bindgen_test]
    fn history_navigation_updates_input_buffer() {
        let mut state = AppState::new();
        state.command_history.push("help".to_string());
        state.command_history.push("faq".to_string());

        let newest = super::select_history_entry(&mut state, HistoryDirection::Older)
            .expect("history should produce newest command");
        assert_eq!(newest, "faq");
        assert_eq!(state.input_buffer, "faq");
        assert_eq!(state.history_index, Some(1));

        let older = super::select_history_entry(&mut state, HistoryDirection::Older)
            .expect("history should produce older command");
        assert_eq!(older, "help");
        assert_eq!(state.input_buffer, "help");
        assert_eq!(state.history_index, Some(0));

        let newer = super::select_history_entry(&mut state, HistoryDirection::Newer)
            .expect("history should move forward");
        assert_eq!(newer, "faq");
        assert_eq!(state.input_buffer, "faq");
        assert_eq!(state.history_index, Some(1));

        let exit = super::select_history_entry(&mut state, HistoryDirection::Newer)
            .expect("history should exit to empty buffer");
        assert_eq!(exit, "");
        assert_eq!(state.input_buffer, "");
        assert_eq!(state.history_index, None);
    }
}
