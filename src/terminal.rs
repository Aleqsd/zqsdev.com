use crate::ai;
use crate::commands::{self, CommandAction, CommandError, PokemonAttemptOutcome};
use crate::renderer::{AchievementView, Renderer, ScrollBehavior};
use crate::state::AppState;
use crate::utils;
use gloo_timers::future::TimeoutFuture;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlElement;

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
const AI_HELP_COMMAND: &str = "help";
const AI_QUIT_COMMAND: &str = "quit";
const AI_QUIT_LABEL: &str = "Quit AI";
const AI_STATUS_ACTIVE: &str = "AI Mode: Activated";
const AI_STATUS_DEACTIVATED: &str = "AI Mode: Deactivated";
const AI_STATUS_BUSY: &str = "AI Mode: Activated — Synthesizing…";
const AI_ACTIVATED_INFO: &str =
    "🤖 AI Mode activated. Ask anything about Alexandre DO-O ALMEIDA's profile.";
const AI_DEACTIVATED_INFO: &str = "📟 AI Mode deactivated. Classic terminal helpers restored.";
const AI_HELP_MESSAGE: &str = "🤖 AI Mode help:\nYou're chatting with an assistant that only uses Alexandre's résumé data.\nAsk a question or type `quit` to exit AI Mode.";
const AI_DATA_LOADING: &str = "AI knowledge base still loading. Please try again shortly.";
const BOOT_SEQUENCE_MESSAGE: &str = "Welcome to the ZQSDev interactive terminal!";
const WELCOME_GUIDANCE_LINES: [&str; 2] = [
    "Type `help` to view all available commands.",
    "Use the quick actions below to jump to key sections instantly.",
];
const TV_OFF_COMMAND: &str = "rm -rf";
const TV_OFF_WARNING: &str = "⚠️ `rm -rf` sequence detected. Powering down terminal…";
const KONAMI_CODE: [&str; 10] = [
    "ArrowUp",
    "ArrowUp",
    "ArrowDown",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "ArrowLeft",
    "ArrowRight",
    "b",
    "a",
];
const KONAMI_ALERT: &str = "🕹️ Cheat code accepted! Goku is charging a Kamehameha!";
const KAMEHAMEHA_MEDIA_HTML: &str = r#"
<figure class="konami-kamehameha">
    <img
        class="konami-kamehameha__video"
        src="./effects/kamehameha.gif"
        alt="Goku unleashes a Kamehameha beam"
        loading="lazy"
    >
    <audio
        class="konami-kamehameha__audio"
        src="./effects/kamehameha.mp3"
        preload="auto"
        autoplay
        playsinline
    ></audio>
</figure>
"#;
const GOKU_FINISHER_HTML: &str =
    r#"<div class="konami-message konami-message--goku">Goku: "KAMEHAMEHA!" 💥</div>"#;
const TERMINAL_EXPLODED_HTML: &str = r#"<div class="konami-message konami-message--terminal">💥 The terminal has exploded. Refresh the page to revive it.</div>"#;
const KAMEHAMEHA_PROMPT_LABEL: &str = "⚡ KI>$";
const ACHIEVEMENT_SHAW_TITLE: &str = "Shaw!";
const ACHIEVEMENT_SHAW_DESCRIPTION: &str = "Could she be... a Hunter?";
const ACHIEVEMENT_POKEMON_TITLE: &str = "Who's that Pokemon?";
const ACHIEVEMENT_POKEMON_DESCRIPTION: &str = "It's Pikachu!";
const ACHIEVEMENT_KONAMI_TITLE: &str = "Kamehameha!";
const ACHIEVEMENT_KONAMI_DESCRIPTION: &str = "And this... is to go even further beyond!";
const ACHIEVEMENT_SHUTDOWN_TITLE: &str = "AAAAAAAAAAAAAH";
const ACHIEVEMENT_SHUTDOWN_DESCRIPTION: &str = "Why would you do that?!";

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

    pub fn focus(&self) {
        if self.input_disabled() {
            return;
        }
        self.renderer.focus_terminal();
    }

    pub fn open_achievements_modal(&self) -> Result<(), JsValue> {
        let achievements = self.collect_achievement_views();
        self.renderer.show_achievements_modal(&achievements)?;
        {
            let mut state = self.state.borrow_mut();
            state.achievements_modal_open = true;
        }
        Ok(())
    }

    pub fn close_achievements_modal(&self) -> Result<(), JsValue> {
        {
            let mut state = self.state.borrow_mut();
            state.achievements_modal_open = false;
        }
        self.renderer.hide_achievements_modal()
    }

    pub fn handle_escape(&self) {
        if self.close_achievements_modal_if_open() {
            return;
        }
        self.clear_input();
    }

    pub fn overwrite_input(&self, value: &str) {
        if self.input_disabled() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.input_buffer = value.to_string();
            state.history_index = None;
        }
        self.refresh_input();
        self.refresh_suggestions();
    }

    pub fn push_system_message(&self, message: &str) {
        let _ = self
            .renderer
            .append_info_line(message, ScrollBehavior::Bottom);
    }

    pub fn submit_command(&self) -> Result<(), JsValue> {
        if self.input_disabled() {
            return Ok(());
        }
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

        let ai_mode_active = self.ai_mode_active();
        let command_scroll = if ai_mode_active {
            ScrollBehavior::Bottom
        } else {
            ScrollBehavior::Anchor
        };
        self.renderer.append_spacer_line(ScrollBehavior::None)?;
        self.renderer
            .append_command(&prompt_label, &display_line, command_scroll)?;

        if trimmed.is_empty() {
            return Ok(());
        }

        if Self::is_shutdown_command(&trimmed) {
            let celebrate = {
                let mut state = self.state.borrow_mut();
                state.unlock_shutdown_protocol()
            };
            if celebrate {
                self.trigger_achievement_popup(
                    ACHIEVEMENT_SHUTDOWN_TITLE,
                    ACHIEVEMENT_SHUTDOWN_DESCRIPTION,
                )?;
                self.refresh_achievements_modal_if_visible()?;
            }

            self.trigger_shutdown_sequence(1000)?;
            return Ok(());
        }

        if ai_mode_active {
            return self.handle_ai_mode_submission(trimmed);
        }

        let args: Vec<&str> = trimmed.split_whitespace().collect();
        let command = args.first().cloned().unwrap_or_default();
        let extra = if args.is_empty() { &[][..] } else { &args[1..] };

        let action = {
            let state = self.state.borrow();
            commands::execute(command, &state, extra)
        };

        let output_scroll = if ai_mode_active {
            ScrollBehavior::Bottom
        } else {
            ScrollBehavior::None
        };

        match action {
            Ok(CommandAction::Output(text)) => {
                self.renderer.append_output_text(&text, output_scroll)?;
            }
            Ok(CommandAction::OutputHtml(html)) => {
                self.renderer.append_output_html(&html, output_scroll)?;
            }
            Ok(CommandAction::ShawEffect) => {
                self.play_shaw_effect()?;
            }
            Ok(CommandAction::PokemonAttempt(outcome)) => {
                self.play_pokemon_attempt(&outcome, output_scroll)?;
            }
            Ok(CommandAction::Clear) => {
                self.renderer.clear_output();
            }
            Ok(CommandAction::Download(url)) => {
                utils::open_link(&url);
                let confirmation = format!("Opening résumé at {url}");
                self.renderer
                    .append_info_line(&confirmation, output_scroll)?;
            }
            Err(CommandError::NotFound { command }) => {
                self.handle_unknown_command(&command)?;
            }
            Err(CommandError::Message(message)) => {
                self.renderer.append_output_text(&message, output_scroll)?;
            }
        }

        Ok(())
    }

    pub fn process_konami_key(&self, key: &str) -> Result<bool, JsValue> {
        let Some(normalized) = Self::normalize_konami_key(key) else {
            self.reset_konami_progress();
            return Ok(false);
        };

        let triggered = {
            let mut state = self.state.borrow_mut();
            if state.konami_triggered {
                false
            } else if KONAMI_CODE[state.konami_index] == normalized {
                state.konami_index += 1;
                if state.konami_index == KONAMI_CODE.len() {
                    state.konami_index = 0;
                    state.konami_triggered = true;
                    true
                } else {
                    false
                }
            } else {
                state.konami_index = if normalized == KONAMI_CODE[0] { 1 } else { 0 };
                false
            }
        };

        if triggered {
            let celebrate = {
                let mut state = self.state.borrow_mut();
                state.unlock_konami_secret()
            };
            self.start_kamehameha_sequence()?;
            if celebrate {
                self.trigger_achievement_popup(
                    ACHIEVEMENT_KONAMI_TITLE,
                    ACHIEVEMENT_KONAMI_DESCRIPTION,
                )?;
                self.refresh_achievements_modal_if_visible()?;
            }
            return Ok(true);
        }

        Ok(false)
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
        let info_scroll = if self.ai_mode_active() {
            ScrollBehavior::Bottom
        } else {
            ScrollBehavior::None
        };
        self.renderer
            .append_output_text(&message, info_scroll.clone())?;
        let html = r#"Need a hand? <button type="button" class="ai-mode-cta" data-action="activate-ai-mode">Ask the AI assistant</button>"#;
        self.renderer.append_info_html(html, info_scroll)?;
        Ok(())
    }

    pub fn clear_input(&self) {
        if self.input_disabled() {
            return;
        }
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
        if self.input_disabled() || value.is_empty() {
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
        if self.input_disabled() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.input_buffer.pop();
            state.history_index = None;
        }
        self.refresh_input();
        self.refresh_suggestions();
    }

    pub fn navigate_history(&self, direction: HistoryDirection) {
        if self.input_disabled() {
            return;
        }
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
        if self.input_disabled() {
            return;
        }
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
        if self.input_disabled() {
            return Ok(());
        }
        {
            let mut state = self.state.borrow_mut();
            state.input_buffer = command.to_string();
            state.history_index = None;
        }
        self.refresh_input();
        self.submit_command()
    }

    pub fn on_data_ready(&self) -> Result<(), JsValue> {
        let (profile_name, resume_link) = {
            let state = self.state.borrow();
            let name = state.data.as_ref().map(|data| data.profile.name.clone());
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
            (name, link)
        };

        let renderer = Rc::clone(&self.renderer);
        spawn_local(async move {
            if let Err(err) = renderer
                .type_output_text(BOOT_SEQUENCE_MESSAGE, WELCOME_TYPE_DELAY_MS)
                .await
            {
                utils::log(&format!("Failed to animate welcome message: {:?}", err));
                if let Err(err) =
                    renderer.append_output_text(BOOT_SEQUENCE_MESSAGE, ScrollBehavior::Bottom)
                {
                    utils::log(&format!(
                        "Failed to render welcome message fallback: {:?}",
                        err
                    ));
                }
            }

            if let Some(name) = profile_name {
                let profile_line = profile_loaded_line(&name);
                if let Err(err) = renderer.append_output_text(&profile_line, ScrollBehavior::Bottom)
                {
                    utils::log(&format!(
                        "Failed to append profile line `{profile_line}`: {:?}",
                        err
                    ));
                }
            }

            for guidance in WELCOME_GUIDANCE_LINES {
                if let Err(err) = renderer.append_info_line(guidance, ScrollBehavior::Bottom) {
                    utils::log(&format!(
                        "Failed to append guidance line `{guidance}`: {:?}",
                        err
                    ));
                }
            }

            let resume_html = resume_link_html(&resume_link);
            if let Err(err) = renderer.append_info_html(&resume_html, ScrollBehavior::Bottom) {
                utils::log(&format!("Failed to append résumé link: {:?}", err));
            }
            let ai_cta_html = r#"Prefer to talk with an AI? <button type="button" class="ai-mode-cta" data-action="activate-ai-mode">Ask the AI assistant</button>"#;
            if let Err(err) = renderer.append_info_html(ai_cta_html, ScrollBehavior::Bottom) {
                utils::log(&format!(
                    "Failed to append AI assistant call-to-action: {:?}",
                    err
                ));
            }
        });

        Ok(())
    }

    fn trigger_shutdown_sequence(&self, delay_ms: u32) -> Result<(), JsValue> {
        if self.ensure_input_disabled() {
            return Ok(());
        }

        self.renderer.disable_prompt_input()?;
        self.renderer.update_input("");
        self.renderer
            .render_suggestions(std::iter::empty::<(String, String)>());

        let renderer = Rc::clone(&self.renderer);
        spawn_local(async move {
            TimeoutFuture::new(delay_ms).await;

            if let Err(err) = renderer.append_info_line(TV_OFF_WARNING, ScrollBehavior::Bottom) {
                utils::log(&format!(
                    "Failed to append shutdown warning line after delay: {:?}",
                    err
                ));
                return;
            }

            if let Err(err) = renderer.play_tv_shutdown_animation() {
                utils::log(&format!(
                    "Failed to play TV shutdown animation after delay: {:?}",
                    err
                ));
            }
        });
        Ok(())
    }

    fn play_pokemon_attempt(
        &self,
        outcome: &PokemonAttemptOutcome,
        behavior: ScrollBehavior,
    ) -> Result<(), JsValue> {
        {
            let mut state = self.state.borrow_mut();
            state.set_pokemon_capture_chance(outcome.next_chance);
        }

        let chance_message = format!(
            "You have a {chance}% chance of catching Pikachu!",
            chance = outcome.current_chance
        );
        self.renderer
            .append_output_text(&chance_message, behavior)?;

        let attempt_effect = self.renderer.render_pokemon_capture_attempt()?;
        self.dismiss_pokemon_effect_after_delay(&attempt_effect, 2000);

        if outcome.success {
            self.renderer
                .append_info_line("⚡️ Poké Ball shakes… success!", ScrollBehavior::Bottom)?;
            let celebrate = {
                let mut state = self.state.borrow_mut();
                state.unlock_pokemon_master()
            };
            if celebrate {
                self.trigger_achievement_popup(
                    ACHIEVEMENT_POKEMON_TITLE,
                    ACHIEVEMENT_POKEMON_DESCRIPTION,
                )?;
                self.refresh_achievements_modal_if_visible()?;
            }
            let success_effect = self.renderer.render_pokemon_capture_success()?;
            self.dismiss_pokemon_effect_after_delay(&success_effect, 5000);
            self.renderer.append_info_line(
                "Pikachu was caught! Congratulations!",
                ScrollBehavior::Bottom,
            )?;
        } else {
            self.renderer
                .append_info_line("Oh you failed, try again!", ScrollBehavior::Bottom)?;
            let state = self.state.borrow();
            let next = state.pokemon_capture_chance();
            drop(state);
            if next > outcome.current_chance {
                let encouragement =
                    format!("Your next capture chance rises to {next}%.", next = next);
                self.renderer
                    .append_info_line(&encouragement, ScrollBehavior::Bottom)?;
            }
        }

        Ok(())
    }

    fn dismiss_pokemon_effect_after_delay(&self, element: &HtmlElement, delay_ms: u32) {
        let renderer = Rc::clone(&self.renderer);
        let element = element.clone();
        spawn_local(async move {
            TimeoutFuture::new(delay_ms).await;

            if let Err(err) = element.set_attribute("data-state", "hiding") {
                utils::log(&format!(
                    "Failed to mark Pokémon effect for dismissal: {:?}",
                    err
                ));
                return;
            }

            TimeoutFuture::new(260).await;

            if let Err(err) = renderer.remove_effect(&element) {
                utils::log(&format!(
                    "Failed to remove Pokémon effect element: {:?}",
                    err
                ));
            }
        });
    }

    fn trigger_achievement_popup(&self, title: &str, description: &str) -> Result<(), JsValue> {
        let toast = self.renderer.render_achievement_toast(title, description)?;
        self.dismiss_achievement_after_delay(&toast, 5200);
        Ok(())
    }

    fn dismiss_achievement_after_delay(&self, toast: &HtmlElement, delay_ms: u32) {
        let renderer = Rc::clone(&self.renderer);
        let toast = toast.clone();
        spawn_local(async move {
            TimeoutFuture::new(delay_ms).await;

            if let Err(err) = toast.set_attribute("data-state", "hiding") {
                utils::log(&format!(
                    "Failed to mark achievement toast for dismissal: {:?}",
                    err
                ));
                return;
            }

            TimeoutFuture::new(320).await;

            if let Err(err) = renderer.remove_effect(&toast) {
                utils::log(&format!(
                    "Failed to remove achievement toast element: {:?}",
                    err
                ));
            }
        });
    }

    fn refresh_achievements_modal_if_visible(&self) -> Result<(), JsValue> {
        let should_refresh = {
            let state = self.state.borrow();
            state.achievements_modal_open
        };
        if !should_refresh {
            return Ok(());
        }
        let achievements = self.collect_achievement_views();
        self.renderer.show_achievements_modal(&achievements)
    }

    fn collect_achievement_views(&self) -> Vec<AchievementView> {
        let (shaw, pokemon, konami, shutdown) = {
            let state = self.state.borrow();
            (
                state.achievement_shaw_unlocked,
                state.achievement_pokemon_unlocked,
                state.achievement_konami_unlocked,
                state.achievement_shutdown_unlocked,
            )
        };

        let mut unlocked = Vec::new();
        let mut locked = Vec::new();
        let entries = [
            (shaw, ACHIEVEMENT_SHAW_TITLE, ACHIEVEMENT_SHAW_DESCRIPTION),
            (
                pokemon,
                ACHIEVEMENT_POKEMON_TITLE,
                ACHIEVEMENT_POKEMON_DESCRIPTION,
            ),
            (
                konami,
                ACHIEVEMENT_KONAMI_TITLE,
                ACHIEVEMENT_KONAMI_DESCRIPTION,
            ),
            (
                shutdown,
                ACHIEVEMENT_SHUTDOWN_TITLE,
                ACHIEVEMENT_SHUTDOWN_DESCRIPTION,
            ),
        ];

        for (is_unlocked, title, description) in entries {
            let view = AchievementView::new(title, description, is_unlocked);
            if is_unlocked {
                unlocked.push(view);
            } else {
                locked.push(view);
            }
        }

        unlocked.extend(locked);
        unlocked
    }

    fn close_achievements_modal_if_open(&self) -> bool {
        let is_open = {
            let state = self.state.borrow();
            state.achievements_modal_open
        };
        if !is_open {
            return false;
        }
        if let Err(err) = self.close_achievements_modal() {
            utils::log(&format!("Failed to close achievements modal: {:?}", err));
        }
        true
    }

    fn play_shaw_effect(&self) -> Result<(), JsValue> {
        let celebrate = {
            let mut state = self.state.borrow_mut();
            state.unlock_shaw_celebration()
        };

        if celebrate {
            self.trigger_achievement_popup(ACHIEVEMENT_SHAW_TITLE, ACHIEVEMENT_SHAW_DESCRIPTION)?;
            self.refresh_achievements_modal_if_visible()?;
        }

        self.renderer.force_scroll_to_bottom();

        let renderer = Rc::clone(&self.renderer);
        spawn_local(async move {
            // Allow the terminal to settle at the bottom before showing the effect.
            TimeoutFuture::new(120).await;

            let effect = match renderer.render_shaw_effect() {
                Ok(effect) => effect,
                Err(err) => {
                    utils::log(&format!("Failed to render Shaw effect: {:?}", err));
                    return;
                }
            };

            renderer.force_scroll_to_bottom();

            TimeoutFuture::new(3000).await;

            if let Err(err) = effect.set_attribute("data-state", "hiding") {
                utils::log(&format!(
                    "Failed to mark Shaw effect for dismissal: {:?}",
                    err
                ));
            }

            TimeoutFuture::new(260).await;

            if let Err(err) = renderer.remove_effect(&effect) {
                utils::log(&format!("Failed to remove Shaw effect: {:?}", err));
            }
        });

        Ok(())
    }

    fn ensure_input_disabled(&self) -> bool {
        let mut state = self.state.borrow_mut();
        if state.input_disabled() {
            true
        } else {
            state.set_input_disabled(true);
            false
        }
    }

    fn start_kamehameha_sequence(&self) -> Result<(), JsValue> {
        if self.ensure_input_disabled() {
            return Ok(());
        }

        self.renderer.disable_prompt_input()?;
        self.renderer.update_input("");
        self.renderer
            .render_suggestions(std::iter::empty::<(String, String)>());
        {
            let mut state = self.state.borrow_mut();
            state.prompt_label = KAMEHAMEHA_PROMPT_LABEL.to_string();
            state.input_buffer.clear();
        }
        self.renderer.set_prompt_label(KAMEHAMEHA_PROMPT_LABEL);
        self.renderer.play_konami_charge()?;

        let renderer = Rc::clone(&self.renderer);
        spawn_local(async move {
            if let Err(err) = renderer.append_info_line(KONAMI_ALERT, ScrollBehavior::Bottom) {
                utils::log(&format!("Failed to announce Konami code: {:?}", err));
            }

            TimeoutFuture::new(420).await;

            if let Err(err) = renderer.append_info_line(
                "Goku warps onto the terminal roof, palms blazing with ki.",
                ScrollBehavior::Bottom,
            ) {
                utils::log(&format!(
                    "Failed to narrate Goku's entrance into the terminal: {:?}",
                    err
                ));
            }

            renderer.force_scroll_to_bottom();

            if let Err(err) =
                renderer.append_output_html(KAMEHAMEHA_MEDIA_HTML, ScrollBehavior::Bottom)
            {
                utils::log(&format!("Failed to render Goku media: {:?}", err));
            }

            if let Err(err) = renderer.append_info_html(GOKU_FINISHER_HTML, ScrollBehavior::Bottom)
            {
                utils::log(&format!(
                    "Failed to render Goku's finishing line before the explosion: {:?}",
                    err
                ));
            }

            TimeoutFuture::new(8600).await;

            if let Err(err) = renderer.clear_konami_media() {
                utils::log(&format!(
                    "Failed to clear Kamehameha media before finishing move: {:?}",
                    err
                ));
            }

            TimeoutFuture::new(360).await;

            if let Err(err) = renderer.trigger_terminal_explosion() {
                utils::log(&format!(
                    "Failed to apply terminal explosion visuals after Konami code: {:?}",
                    err
                ));
            }

            TimeoutFuture::new(420).await;

            if let Err(err) =
                renderer.append_info_html(TERMINAL_EXPLODED_HTML, ScrollBehavior::Bottom)
            {
                utils::log(&format!(
                    "Failed to render terminal explosion aftermath message: {:?}",
                    err
                ));
            }
        });

        Ok(())
    }

    fn input_disabled(&self) -> bool {
        self.state.borrow().input_disabled()
    }

    fn is_shutdown_command(input: &str) -> bool {
        let mut normalized = String::new();
        for part in input.split_whitespace() {
            if !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.push_str(&part.to_ascii_lowercase());
        }
        normalized.contains(TV_OFF_COMMAND)
    }

    fn refresh_input(&self) {
        let buffer = { self.state.borrow().input_buffer.clone() };
        self.renderer.update_input(&buffer);
    }

    fn refresh_suggestions(&self) {
        if self.input_disabled() {
            self.renderer
                .render_suggestions(std::iter::empty::<(String, String)>());
            return;
        }
        render_current_suggestions(&self.state, &self.renderer);
    }

    fn handle_ai_mode_submission(&self, input: String) -> Result<(), JsValue> {
        let normalized = input.trim().to_ascii_lowercase();
        if normalized == "help" {
            let behavior = if self.ai_mode_active() {
                ScrollBehavior::Bottom
            } else {
                ScrollBehavior::None
            };
            self.renderer
                .append_output_text(AI_HELP_MESSAGE, behavior)?;
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
            let behavior = if self.ai_mode_active() {
                ScrollBehavior::Bottom
            } else {
                ScrollBehavior::None
            };
            self.renderer.append_info_line(AI_DATA_LOADING, behavior)?;
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
                        {
                            let mut state = shared_state.borrow_mut();
                            state.set_ai_model(payload.model.clone());
                        }
                        render_current_suggestions(&shared_state, &renderer);
                        renderer.set_ai_indicator_text(AI_STATUS_ACTIVE);
                        if let Err(err) =
                            renderer.append_output_markdown(&payload.answer, ScrollBehavior::Bottom)
                        {
                            utils::log(&format!("Failed to render AI answer: {:?}", err));
                        }
                    } else {
                        {
                            let mut state = shared_state.borrow_mut();
                            state.set_ai_model(payload.model.clone());
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
                        if let Err(err) = renderer.append_info_line(&notice, ScrollBehavior::Bottom)
                        {
                            utils::log(&format!("Failed to render AI limit info: {:?}", err));
                        }
                    }
                }
                Err(error) => {
                    let message = format!("AI error: {error}");
                    if let Err(err) = renderer.append_output_text(&message, ScrollBehavior::Bottom)
                    {
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
            let behavior = if active {
                ScrollBehavior::Bottom
            } else {
                ScrollBehavior::None
            };
            self.renderer.append_info_line(message, behavior)?;
        }

        if previous != active {
            self.refresh_suggestions();
        }

        Ok(())
    }

    fn ai_mode_active(&self) -> bool {
        self.state.borrow().ai_mode
    }

    fn reset_konami_progress(&self) {
        let mut state = self.state.borrow_mut();
        if !state.konami_triggered {
            state.konami_index = 0;
        }
    }

    fn normalize_konami_key(key: &str) -> Option<&'static str> {
        match key {
            "ArrowUp" => Some("ArrowUp"),
            "ArrowDown" => Some("ArrowDown"),
            "ArrowLeft" => Some("ArrowLeft"),
            "ArrowRight" => Some("ArrowRight"),
            "a" | "A" => Some("a"),
            "b" | "B" => Some("b"),
            _ => None,
        }
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

const HIDDEN_HELPER_COMMANDS: [&str; 2] = ["shaw", "pokemon"];

fn is_hidden_helper(command: &str) -> bool {
    HIDDEN_HELPER_COMMANDS
        .iter()
        .any(|hidden| hidden.eq_ignore_ascii_case(command))
}

fn default_suggestions() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = commands::command_names()
        .into_iter()
        .filter(|name| !is_hidden_helper(name))
        .collect();
    if let Some(index) = names.iter().position(|name| *name == "resume") {
        let resume = names.remove(index);
        names.insert(0, resume);
    }
    names
}

fn ai_help_label(model: Option<&str>) -> String {
    match model {
        Some(name) if !name.trim().is_empty() => format!("AI help ({name})"),
        _ => "AI help".to_string(),
    }
}

fn ai_mode_suggestions(filter: &str, model: Option<&str>) -> Vec<(String, String)> {
    let commands = [
        (AI_HELP_COMMAND, ai_help_label(model)),
        (AI_QUIT_COMMAND, AI_QUIT_LABEL.to_string()),
    ];

    commands
        .into_iter()
        .filter(|(command, _)| filter.is_empty() || command.starts_with(filter))
        .map(|(command, label)| ((*command).to_string(), label))
        .collect()
}

fn render_current_suggestions(state: &SharedState, renderer: &SharedRenderer) {
    let (buffer, ai_mode, ai_model) = {
        let state = state.borrow();
        (
            state.input_buffer.clone(),
            state.ai_mode,
            state.ai_model.clone(),
        )
    };
    let trimmed = buffer.trim().to_ascii_lowercase();

    let suggestions: Vec<(String, String)> = if ai_mode {
        ai_mode_suggestions(&trimmed, ai_model.as_deref())
    } else {
        let names: Vec<String> = if trimmed.is_empty() {
            default_suggestions()
                .into_iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            commands::suggestions(&buffer)
                .into_iter()
                .filter(|name| !is_hidden_helper(name))
                .map(|s| s.to_string())
                .collect()
        };

        names
            .into_iter()
            .filter(|command| !is_hidden_helper(command))
            .map(|command| {
                let label = commands::helper_label(&command);
                (command, label)
            })
            .collect()
    };

    renderer.render_suggestions(suggestions);
}

fn profile_loaded_line(name: &str) -> String {
    format!("Profile loaded for {}.", name)
}

fn resume_link_html(url: &str) -> String {
    format!(
        r#"If you want, you can just <a href="{url}" target="_blank" rel="noopener noreferrer">open the résumé</a>."#,
        url = url
    )
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

    #[test]
    fn boot_sequence_matches_spec() {
        assert_eq!(
            super::BOOT_SEQUENCE_MESSAGE,
            "Welcome to the ZQSDev interactive terminal!"
        );
    }

    #[test]
    fn guidance_lines_match_spec() {
        assert_eq!(
            super::WELCOME_GUIDANCE_LINES,
            [
                "Type `help` to view all available commands.",
                "Use the quick actions below to jump to key sections instantly."
            ]
        );
    }

    #[test]
    fn profile_loaded_line_formats_name() {
        assert_eq!(
            super::profile_loaded_line("Alex"),
            "Profile loaded for Alex."
        );
    }

    #[test]
    fn resume_link_html_wraps_anchor() {
        let html = super::resume_link_html("https://cv.zqsdev.com");
        assert!(
            html.contains(r#"href="https://cv.zqsdev.com""#),
            "Expected résumé anchor to include the provided URL: {html}"
        );
        assert!(
            html.contains("open the résumé"),
            "Anchor markup should include the résumé label: {html}"
        );
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

    #[test]
    fn ai_help_label_includes_model_name() {
        let label = super::ai_help_label(Some("gpt-4o-mini"));
        assert!(
            label.contains("gpt-4o-mini"),
            "AI help label should include the model name: {label}"
        );
    }

    #[test]
    fn ai_mode_suggestions_label_help_with_model() {
        let suggestions = super::ai_mode_suggestions("", Some("llama-3.1-8b-instant"));
        let help = suggestions
            .iter()
            .find(|(command, _)| command == "help")
            .expect("help suggestion missing");
        assert!(
            help.1.contains("llama-3.1-8b-instant"),
            "Help label should mention the active model {help:?}"
        );
    }

    #[test]
    fn ai_mode_suggestions_filter_by_prefix() {
        let suggestions = super::ai_mode_suggestions("q", Some("gpt"));
        assert_eq!(
            suggestions.len(),
            1,
            "Only the quit suggestion should match prefix `q`"
        );
        assert_eq!(suggestions[0].0, "quit");
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
