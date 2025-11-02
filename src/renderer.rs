use crate::keyword_icons::{self, Segment as KeywordSegment};
use crate::markdown;
use crate::utils;
use gloo_timers::future::TimeoutFuture;
use js_sys::Math;
use std::cell::RefCell;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    Document, DocumentFragment, Element, HtmlAudioElement, HtmlButtonElement, HtmlDivElement,
    HtmlElement, HtmlImageElement, HtmlInputElement, HtmlSpanElement, Node, Text,
};

const TERMINAL_ID: &str = "terminal";
const OUTPUT_ID: &str = "output";
const PROMPT_INPUT_ID: &str = "prompt-input";
const PROMPT_HIDDEN_INPUT_ID: &str = "prompt-hidden-input";
const PROMPT_LABEL_ID: &str = "prompt-label";
const SUGGESTIONS_ID: &str = "suggestions";
const AI_TOGGLE_ID: &str = "ai-mode-toggle";
const AI_INDICATOR_ID: &str = "ai-mode-indicator";
const AI_LOADER_ID: &str = "ai-loader";

const COMPACT_SUGGESTION_VISIBLE_COUNT: usize = 4;
const SUGGESTION_EXPAND_LABEL: &str = "Show more";
const SUGGESTION_COLLAPSE_LABEL: &str = "Show less";

#[derive(Clone, Copy)]
pub enum ScrollBehavior {
    None,
    Anchor,
    Bottom,
}

#[derive(Clone, Debug)]
pub struct AchievementView {
    pub title: String,
    pub description: String,
    pub hint: String,
    pub unlocked: bool,
}

impl AchievementView {
    pub fn new(title: &str, description: &str, hint: &str, unlocked: bool) -> Self {
        Self {
            title: title.to_string(),
            description: description.to_string(),
            hint: hint.to_string(),
            unlocked,
        }
    }
}

#[derive(Clone)]
pub struct CookieClickerView {
    pub line: HtmlElement,
    pub wrapper: HtmlElement,
    pub button: HtmlButtonElement,
    pub counter: HtmlElement,
    pub prompt: HtmlElement,
}

pub struct Renderer {
    document: Document,
    terminal_root: HtmlElement,
    output: HtmlElement,
    prompt_input: HtmlElement,
    prompt_hidden_input: HtmlInputElement,
    prompt_label: HtmlElement,
    suggestions: HtmlElement,
    ai_toggle: HtmlElement,
    ai_indicator: HtmlElement,
    achievement_layer: HtmlElement,
    achievements_trigger: HtmlElement,
    achievements_overlay: HtmlElement,
    achievements_modal: HtmlElement,
    last_command: RefCell<Option<HtmlElement>>,
}

impl Renderer {
    pub fn new() -> Result<Self, JsValue> {
        let document = utils::document()?;
        let terminal_root = get_html_element(&document, TERMINAL_ID)?;
        let output = get_html_element(&document, OUTPUT_ID)?;
        let prompt_input = get_html_element(&document, PROMPT_INPUT_ID)?;
        let prompt_hidden_input =
            get_html_element(&document, PROMPT_HIDDEN_INPUT_ID)?.dyn_into::<HtmlInputElement>()?;
        let prompt_label = get_html_element(&document, PROMPT_LABEL_ID)?;
        let suggestions = get_html_element(&document, SUGGESTIONS_ID)?;
        let ai_toggle = get_html_element(&document, AI_TOGGLE_ID)?;
        let ai_indicator = get_html_element(&document, AI_INDICATOR_ID)?;
        let achievements_trigger = get_html_element(&document, "achievements-trigger")?;
        let achievements_overlay = get_html_element(&document, "achievements-overlay")?;
        let achievements_modal = get_html_element(&document, "achievements-modal")?;
        achievements_trigger.set_attribute("aria-expanded", "false")?;
        achievements_overlay.set_attribute("data-state", "hidden")?;
        achievements_overlay.set_attribute("aria-hidden", "true")?;
        let achievement_layer = match terminal_root
            .query_selector(".achievement-layer")?
            .map(|node| node.dyn_into::<HtmlElement>())
        {
            Some(Ok(existing)) => existing,
            Some(Err(_)) => {
                let layer = document.create_element("div")?.dyn_into::<HtmlElement>()?;
                layer.set_class_name("achievement-layer");
                terminal_root.append_child(&layer)?;
                layer
            }
            None => {
                let layer = document.create_element("div")?.dyn_into::<HtmlElement>()?;
                layer.set_class_name("achievement-layer");
                terminal_root.append_child(&layer)?;
                layer
            }
        };

        Ok(Self {
            document,
            terminal_root,
            output,
            prompt_input,
            prompt_hidden_input,
            prompt_label,
            suggestions,
            ai_toggle,
            ai_indicator,
            achievement_layer,
            achievements_trigger,
            achievements_overlay,
            achievements_modal,
            last_command: RefCell::new(None),
        })
    }

    pub fn set_prompt_label(&self, label: &str) {
        self.prompt_label.set_text_content(Some(label));
    }

    pub fn update_input(&self, buffer: &str) {
        self.prompt_input.set_text_content(Some(buffer));
        self.prompt_hidden_input.set_value(buffer);
        let end = buffer.encode_utf16().count() as u32;
        let _ = self.prompt_hidden_input.set_selection_range(end, end);
    }

    pub fn focus_terminal(&self) {
        let _ = self.prompt_hidden_input.focus();
        let end = self.prompt_hidden_input.value().encode_utf16().count() as u32;
        let _ = self.prompt_hidden_input.set_selection_range(end, end);
    }

    pub fn append_command(
        &self,
        label: &str,
        command: &str,
        behavior: ScrollBehavior,
    ) -> Result<(), JsValue> {
        let line = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlElement>()?;
        line.set_class_name("line command-line");

        let label_span = self
            .document
            .create_element("span")?
            .dyn_into::<HtmlSpanElement>()?;
        label_span.set_class_name("prompt-label");
        label_span.set_text_content(Some(label));

        let command_span = self
            .document
            .create_element("span")?
            .dyn_into::<HtmlSpanElement>()?;
        command_span.set_class_name("prompt-command");
        command_span.set_text_content(Some(command));

        line.append_child(&label_span)?;
        line.append_child(&command_span)?;
        self.output.append_child(&line)?;
        {
            let mut anchor = self.last_command.borrow_mut();
            *anchor = Some(line.clone());
        }
        let element: &HtmlElement = line.unchecked_ref();
        self.apply_scroll(element, behavior)?;
        Ok(())
    }

    pub fn append_spacer_line(&self, behavior: ScrollBehavior) -> Result<(), JsValue> {
        let spacer = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlElement>()?;
        spacer.set_class_name("line spacer-line");
        spacer.set_text_content(Some("\u{00a0}"));
        spacer.set_attribute("aria-hidden", "true")?;
        self.output.append_child(&spacer)?;
        let element: &HtmlElement = spacer.unchecked_ref();
        self.apply_scroll(element, behavior)?;
        Ok(())
    }

    pub fn append_output_text(&self, text: &str, behavior: ScrollBehavior) -> Result<(), JsValue> {
        let wrapper = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlDivElement>()?;
        wrapper.set_class_name("line output-text");

        let pre = self
            .document
            .create_element("pre")?
            .dyn_into::<HtmlElement>()?;
        pre.set_class_name("output-block");
        self.render_text_with_icons(&pre, text)?;

        wrapper.append_child(&pre)?;
        self.output.append_child(&wrapper)?;
        let element: &HtmlElement = wrapper.unchecked_ref();
        self.apply_scroll(element, behavior)?;
        Ok(())
    }

    pub fn show_achievements_modal(
        &self,
        achievements: &[AchievementView],
        spoilers_enabled: bool,
    ) -> Result<(), JsValue> {
        clear_children(&self.achievements_modal)?;
        let spoilers_state = if spoilers_enabled {
            "revealed"
        } else {
            "hidden"
        };
        self.achievements_overlay
            .set_attribute("data-spoilers", spoilers_state)?;
        self.achievements_modal
            .set_attribute("data-spoilers", spoilers_state)?;

        let header = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlElement>()?;
        header.set_class_name("achievements-modal__header");

        let title_el = self
            .document
            .create_element("h2")?
            .dyn_into::<HtmlElement>()?;
        title_el.set_id("achievements-modal-title");
        title_el.set_class_name("achievements-modal__title");
        title_el.set_text_content(Some("Easter Eggs"));

        let actions = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlElement>()?;
        actions.set_class_name("achievements-modal__actions");

        let spoiler_btn = self
            .document
            .create_element("button")?
            .dyn_into::<HtmlButtonElement>()?;
        spoiler_btn.set_class_name("achievements-modal__action");
        spoiler_btn.set_attribute("type", "button")?;
        spoiler_btn.set_attribute("data-role", "achievements-spoilers")?;
        spoiler_btn.set_attribute(
            "aria-pressed",
            if spoilers_enabled { "true" } else { "false" },
        )?;
        spoiler_btn.set_text_content(Some(if spoilers_enabled {
            "Hide spoilers"
        } else {
            "Reveal spoilers"
        }));
        actions.append_child(&spoiler_btn)?;

        let reset_btn = self
            .document
            .create_element("button")?
            .dyn_into::<HtmlButtonElement>()?;
        reset_btn.set_class_name("achievements-modal__action");
        reset_btn.set_attribute("type", "button")?;
        reset_btn.set_attribute("data-role", "achievements-reset")?;
        reset_btn.set_text_content(Some("Reset"));
        actions.append_child(&reset_btn)?;

        let close_btn = self
            .document
            .create_element("button")?
            .dyn_into::<HtmlButtonElement>()?;
        close_btn.set_class_name("achievements-modal__action achievements-modal__close");
        close_btn.set_attribute("type", "button")?;
        close_btn.set_attribute("data-role", "achievements-close")?;
        close_btn.set_attribute("aria-label", "Close Easter Eggs panel")?;
        close_btn.set_text_content(Some("Close"));

        header.append_child(&title_el)?;
        header.append_child(&actions)?;
        actions.append_child(&close_btn)?;
        self.achievements_modal.append_child(&header)?;

        let unlocked_count = achievements.iter().filter(|entry| entry.unlocked).count();
        let total_count = achievements.len();

        let summary = self
            .document
            .create_element("p")?
            .dyn_into::<HtmlElement>()?;
        summary.set_class_name("achievements-modal__summary");
        summary.set_text_content(Some(&format!(
            "Easter eggs found: {unlocked}/{total}",
            unlocked = unlocked_count,
            total = total_count
        )));
        self.achievements_modal.append_child(&summary)?;

        let hint = self
            .document
            .create_element("p")?
            .dyn_into::<HtmlElement>()?;
        hint.set_class_name("achievements-modal__hint");
        hint.set_text_content(Some(
            "Hover an Easter egg to uncover a clue about how to trigger it.",
        ));
        self.achievements_modal.append_child(&hint)?;

        let list = self
            .document
            .create_element("ul")?
            .dyn_into::<HtmlElement>()?;
        list.set_class_name("achievements-modal__list");

        for achievement in achievements {
            let item = self
                .document
                .create_element("li")?
                .dyn_into::<HtmlElement>()?;
            item.set_class_name("achievement-card");
            item.set_attribute(
                "data-state",
                if achievement.unlocked {
                    "unlocked"
                } else {
                    "locked"
                },
            )?;
            item.set_attribute("data-hint", &achievement.hint)?;
            item.set_attribute("tabindex", "0")?;

            let reveal_details = spoilers_enabled || achievement.unlocked;
            let title_text = if reveal_details {
                achievement.title.clone()
            } else {
                "Hidden Easter egg".to_string()
            };
            let description_text = if reveal_details {
                achievement.description.clone()
            } else {
                "Find this Easter egg to reveal the story.".to_string()
            };

            let summary = self
                .document
                .create_element("div")?
                .dyn_into::<HtmlElement>()?;
            summary.set_class_name("achievement-card__summary");

            let icon = self
                .document
                .create_element("span")?
                .dyn_into::<HtmlElement>()?;
            icon.set_class_name("achievement-card__icon");
            icon.set_text_content(Some("🥚"));

            let meta = self
                .document
                .create_element("div")?
                .dyn_into::<HtmlElement>()?;
            meta.set_class_name("achievement-card__meta");

            let status = self
                .document
                .create_element("span")?
                .dyn_into::<HtmlElement>()?;
            status.set_class_name("achievement-card__status");
            status.set_text_content(Some(if achievement.unlocked {
                "Unlocked"
            } else {
                "Locked"
            }));

            let title = self
                .document
                .create_element("h3")?
                .dyn_into::<HtmlElement>()?;
            title.set_class_name("achievement-card__title");
            title.set_text_content(Some(&title_text));

            let description = self
                .document
                .create_element("p")?
                .dyn_into::<HtmlElement>()?;
            description.set_class_name("achievement-card__description");
            description.set_text_content(Some(&description_text));

            meta.append_child(&status)?;
            meta.append_child(&title)?;
            summary.append_child(&icon)?;
            summary.append_child(&meta)?;

            item.append_child(&summary)?;
            item.append_child(&description)?;
            list.append_child(&item)?;
        }

        self.achievements_modal.append_child(&list)?;

        self.achievements_overlay
            .set_attribute("data-state", "visible")?;
        self.achievements_overlay
            .set_attribute("aria-hidden", "false")?;
        self.achievements_trigger
            .set_attribute("aria-expanded", "true")?;

        if let Err(err) = self.achievements_modal.focus() {
            utils::log(&format!("Failed to focus achievements modal: {:?}", err));
        }

        Ok(())
    }

    pub fn hide_achievements_modal(&self) -> Result<(), JsValue> {
        self.achievements_overlay
            .set_attribute("data-state", "hidden")?;
        self.achievements_overlay
            .set_attribute("aria-hidden", "true")?;
        self.achievements_trigger
            .set_attribute("aria-expanded", "false")?;
        Ok(())
    }

    pub fn append_output_html(&self, html: &str, behavior: ScrollBehavior) -> Result<(), JsValue> {
        let wrapper = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlDivElement>()?;
        wrapper.set_class_name("line output-text");

        let container = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlElement>()?;
        container.set_class_name("output-block output-block--html");
        container.set_inner_html(html);
        self.decorate_with_icons(&container)?;

        wrapper.append_child(&container)?;
        self.output.append_child(&wrapper)?;
        let element: &HtmlElement = wrapper.unchecked_ref();
        self.apply_scroll(element, behavior)?;
        Ok(())
    }

    pub fn append_info_line(&self, message: &str, behavior: ScrollBehavior) -> Result<(), JsValue> {
        let line = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlDivElement>()?;
        line.set_class_name("line info-line");
        self.render_text_with_icons(&line, message)?;
        self.output.append_child(&line)?;
        let element: &HtmlElement = line.unchecked_ref();
        self.apply_scroll(element, behavior)?;
        Ok(())
    }

    pub fn append_info_html(&self, message: &str, behavior: ScrollBehavior) -> Result<(), JsValue> {
        let line = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlDivElement>()?;
        line.set_class_name("line info-line info-neutral");
        line.set_inner_html(message);
        self.decorate_with_icons(&line)?;
        self.output.append_child(&line)?;
        let element: &HtmlElement = line.unchecked_ref();
        self.apply_scroll(element, behavior)?;
        Ok(())
    }

    pub fn append_output_markdown(
        &self,
        text: &str,
        behavior: ScrollBehavior,
    ) -> Result<(), JsValue> {
        let html = markdown::to_html(text);
        self.append_output_html(&html, behavior)
    }

    fn decorate_with_icons(&self, element: &HtmlElement) -> Result<(), JsValue> {
        let node: &Node = element.unchecked_ref();
        self.decorate_node(node)
    }

    fn decorate_node(&self, node: &Node) -> Result<(), JsValue> {
        let children = node.child_nodes();
        let mut text_nodes = Vec::new();
        for idx in 0..children.length() {
            if let Some(child) = children.item(idx) {
                if child.node_type() == Node::TEXT_NODE {
                    if let Ok(text) = child.dyn_into::<Text>() {
                        text_nodes.push(text);
                    }
                } else {
                    if let Some(element) = child.dyn_ref::<Element>() {
                        if element.class_list().contains("keyword-icon") {
                            continue;
                        }
                    }
                    self.decorate_node(&child)?;
                }
            }
        }

        for text_node in text_nodes {
            self.decorate_text_node(&text_node)?;
        }

        Ok(())
    }

    fn decorate_text_node(&self, text_node: &Text) -> Result<(), JsValue> {
        if let Some(parent) = text_node.parent_element() {
            if parent.class_list().contains("keyword-icon") {
                return Ok(());
            }
        }

        let data = text_node.data();
        let segments = keyword_icons::tokenize(&data);
        if !segments
            .iter()
            .any(|segment| matches!(segment, KeywordSegment::Icon(_)))
        {
            return Ok(());
        }

        let fragment: DocumentFragment = self.document.create_document_fragment();
        for segment in segments {
            match segment {
                KeywordSegment::Text(text) => {
                    if text.is_empty() {
                        continue;
                    }
                    let text_node = self.document.create_text_node(&text);
                    let node: Node = text_node.into();
                    fragment.append_child(&node)?;
                }
                KeywordSegment::Icon(icon) => {
                    let span_node = self.build_icon_span(&icon)?;
                    fragment.append_child(&span_node)?;
                }
            }
        }

        let replacement: Node = fragment.into();
        let parent = text_node
            .parent_node()
            .ok_or_else(|| JsValue::from_str("Text node missing parent while decorating icons"))?;
        let original: Node = text_node.clone().into();
        parent.replace_child(&replacement, &original)?;
        Ok(())
    }

    fn render_text_with_icons(&self, element: &HtmlElement, text: &str) -> Result<(), JsValue> {
        let segments = keyword_icons::tokenize(text);
        if !segments
            .iter()
            .any(|segment| matches!(segment, KeywordSegment::Icon(_)))
        {
            element.set_text_content(Some(text));
            return Ok(());
        }

        element.set_text_content(None);
        for segment in segments {
            match segment {
                KeywordSegment::Text(content) => {
                    if content.is_empty() {
                        continue;
                    }
                    let node: Node = self.document.create_text_node(&content).into();
                    element.append_child(&node)?;
                }
                KeywordSegment::Icon(icon) => {
                    let node = self.build_icon_span(&icon)?;
                    element.append_child(&node)?;
                }
            }
        }
        Ok(())
    }

    fn build_icon_span(&self, icon: &keyword_icons::IconMatch) -> Result<Node, JsValue> {
        let span = self
            .document
            .create_element("span")?
            .dyn_into::<HtmlSpanElement>()?;
        span.set_class_name("keyword-icon");

        let image = self
            .document
            .create_element("img")?
            .dyn_into::<HtmlImageElement>()?;
        image.set_class_name("keyword-icon__image");
        image.set_src(icon.icon_path);
        image.set_alt("");
        image.set_attribute("aria-hidden", "true")?;
        image.set_attribute("loading", "lazy")?;
        let image_node: Node = image.into();
        span.append_child(&image_node)?;

        let label_node: Node = self.document.create_text_node(&icon.token).into();
        span.append_child(&label_node)?;

        Ok(span.into())
    }

    pub fn clear_output(&self) {
        self.output.set_inner_html("");
        self.last_command.borrow_mut().take();
    }

    pub async fn type_output_text(&self, text: &str, delay_ms: u32) -> Result<(), JsValue> {
        let wrapper = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlDivElement>()?;
        wrapper.set_class_name("line output-text");

        let pre = self
            .document
            .create_element("pre")?
            .dyn_into::<HtmlElement>()?;
        pre.set_class_name("output-block");

        wrapper.append_child(&pre)?;
        self.output.append_child(&wrapper)?;

        let mut buffer = String::new();
        for ch in text.chars() {
            buffer.push(ch);
            pre.set_text_content(Some(&buffer));
            self.scroll_to_bottom();
            if delay_ms > 0 {
                TimeoutFuture::new(delay_ms).await;
            }
        }
        self.render_text_with_icons(&pre, text)?;
        self.scroll_to_bottom();

        Ok(())
    }

    pub fn render_suggestions<T>(&self, suggestions: T)
    where
        T: IntoIterator<Item = (String, String)>,
    {
        self.suggestions.set_inner_html("");
        let items: Vec<(String, String)> = suggestions.into_iter().collect();
        let total = items.len();
        let has_extras = total > COMPACT_SUGGESTION_VISIBLE_COUNT;
        let expanded = self
            .suggestions
            .get_attribute("data-expanded")
            .map(|value| value == "true")
            .unwrap_or(false);

        let fragment = self.document.create_document_fragment();
        for (index, (command, label)) in items.into_iter().enumerate() {
            if let Ok(div) = self.document.create_element("span") {
                let span = div.dyn_into::<HtmlSpanElement>().ok();
                if let Some(span) = span {
                    let mut classes = String::from("suggestion");
                    if has_extras && index >= COMPACT_SUGGESTION_VISIBLE_COUNT {
                        classes.push_str(" suggestion--extra");
                    }
                    span.set_class_name(&classes);
                    let _ = span.set_attribute("data-command", &command);
                    let _ = span.set_attribute("role", "button");
                    let _ = span.set_attribute("tabindex", "0");
                    span.set_text_content(Some(&label));
                    let _ = fragment.append_child(&span);
                }
            }
        }
        let _ = self.suggestions.append_child(&fragment);

        if has_extras {
            let _ = self
                .suggestions
                .set_attribute("data-expanded", if expanded { "true" } else { "false" });
            let _ = self.suggestions.set_attribute("data-collapsible", "true");

            if let Ok(toggle_el) = self.document.create_element("button") {
                if let Ok(button) = toggle_el.dyn_into::<HtmlButtonElement>() {
                    button.set_class_name("suggestions__toggle");
                    button.set_type("button");
                    button.set_text_content(Some(if expanded {
                        SUGGESTION_COLLAPSE_LABEL
                    } else {
                        SUGGESTION_EXPAND_LABEL
                    }));
                    let _ = button
                        .set_attribute("aria-expanded", if expanded { "true" } else { "false" });
                    let _ = button.set_attribute("aria-controls", SUGGESTIONS_ID);

                    let suggestions_ref = self.suggestions.clone();
                    let button_ref = button.clone();
                    let toggle_handler =
                        Closure::wrap(Box::new(move |event: web_sys::MouseEvent| {
                            event.prevent_default();
                            event.stop_propagation();

                            let is_expanded = suggestions_ref
                                .get_attribute("data-expanded")
                                .map(|value| value == "true")
                                .unwrap_or(false);
                            let next_state = !is_expanded;
                            let _ = suggestions_ref.set_attribute(
                                "data-expanded",
                                if next_state { "true" } else { "false" },
                            );
                            button_ref.set_text_content(Some(if next_state {
                                SUGGESTION_COLLAPSE_LABEL
                            } else {
                                SUGGESTION_EXPAND_LABEL
                            }));
                            let _ = button_ref.set_attribute(
                                "aria-expanded",
                                if next_state { "true" } else { "false" },
                            );
                        }) as Box<dyn FnMut(_)>);

                    let _ = button.add_event_listener_with_callback(
                        "click",
                        toggle_handler.as_ref().unchecked_ref(),
                    );
                    toggle_handler.forget();

                    let _ = self.suggestions.append_child(&button);
                }
            }
        } else {
            let _ = self.suggestions.remove_attribute("data-expanded");
            let _ = self.suggestions.remove_attribute("data-collapsible");
        }
    }

    pub fn disable_prompt_input(&self) -> Result<(), JsValue> {
        self.prompt_hidden_input.set_disabled(true);
        let _ = self.prompt_hidden_input.blur();
        self.prompt_hidden_input
            .set_attribute("aria-disabled", "true")?;
        self.prompt_input.set_attribute("data-disabled", "true")?;
        Ok(())
    }

    pub fn play_konami_charge(&self) -> Result<(), JsValue> {
        let classes = self.terminal_root.class_list();
        let _ = classes.remove_1("ai-mode-active");
        classes.add_1("konami-charge")?;
        Ok(())
    }

    pub fn clear_konami_media(&self) -> Result<(), JsValue> {
        let figure = match self.output.query_selector(".konami-kamehameha") {
            Ok(Some(element)) => element,
            Ok(None) => return Ok(()),
            Err(err) => return Err(err),
        };

        let mut current = figure;
        // Climb up to the wrapper that lives directly under #output.
        loop {
            if let Some(parent) = current.parent_element() {
                if parent.class_list().contains("line") {
                    let node: Node = parent.into();
                    let _ = self.output.remove_child(&node)?;
                    break;
                } else {
                    current = parent;
                    continue;
                }
            } else {
                // Fallback: remove the original figure if we cannot find the wrapper.
                let node: Node = current.into();
                let _ = self.output.remove_child(&node)?;
                break;
            }
        }
        Ok(())
    }

    pub fn trigger_terminal_explosion(&self) -> Result<(), JsValue> {
        let classes = self.terminal_root.class_list();
        let _ = classes.remove_1("konami-charge");
        let _ = classes.remove_1("ai-mode-active");
        classes.add_1("terminal-exploded")?;
        self.terminal_root.set_attribute("data-power", "ko")?;
        self.terminal_root.set_attribute("aria-disabled", "true")?;
        Ok(())
    }

    pub fn play_tv_shutdown_animation(&self) -> Result<(), JsValue> {
        let _ = self.terminal_root.class_list().remove_1("ai-mode-active");
        self.terminal_root.class_list().add_1("tv-off")?;
        self.terminal_root.set_attribute("data-power", "off")?;
        self.terminal_root.set_attribute("aria-disabled", "true")?;
        Ok(())
    }

    pub fn force_scroll_to_bottom(&self) {
        self.scroll_to_bottom();
    }

    pub fn render_shaw_effect(&self) -> Result<HtmlElement, JsValue> {
        let wrapper = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlDivElement>()?;
        wrapper.set_class_name("line output-text shaw-effect-line");

        let figure = self
            .document
            .create_element("figure")?
            .dyn_into::<HtmlElement>()?;
        figure.set_class_name("shaw-effect");

        let image = self
            .document
            .create_element("img")?
            .dyn_into::<HtmlImageElement>()?;
        image.set_class_name("shaw-effect__image");
        image.set_src("./effects/shaw.gif");
        image.set_alt("Shaw bursts onto the terminal stage");
        image.set_attribute("loading", "lazy")?;

        let audio = self
            .document
            .create_element("audio")?
            .dyn_into::<HtmlAudioElement>()?;
        audio.set_class_name("shaw-effect__audio");
        audio.set_src("./effects/shaw.mp3");
        audio.set_preload("auto");
        audio.set_autoplay(true);
        let _ = audio.set_attribute("playsinline", "true");

        let image_node: Node = image.into();
        figure.append_child(&image_node)?;
        let audio_node: Node = audio.clone().into();
        figure.append_child(&audio_node)?;

        let figure_node: Node = figure.into();
        wrapper.append_child(&figure_node)?;
        self.output.append_child(&wrapper)?;

        if let Err(err) = audio.play() {
            utils::log(&format!("Failed to autoplay Shaw audio: {:?}", err));
        }

        let element: HtmlElement = wrapper.clone().dyn_into::<HtmlElement>()?;
        self.apply_scroll(&element, ScrollBehavior::Bottom)?;
        Ok(element)
    }

    pub fn render_achievement_toast(
        &self,
        title: &str,
        description: &str,
    ) -> Result<HtmlElement, JsValue> {
        let toast = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlElement>()?;
        toast.set_class_name("achievement-toast");
        toast.set_attribute("role", "status")?;
        toast.set_attribute("aria-live", "polite")?;

        let icon = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlElement>()?;
        icon.set_class_name("achievement-toast__icon");
        icon.set_text_content(Some("🥚"));

        let content = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlElement>()?;
        content.set_class_name("achievement-toast__content");

        let title_el = self
            .document
            .create_element("p")?
            .dyn_into::<HtmlElement>()?;
        title_el.set_class_name("achievement-toast__title");
        title_el.set_text_content(Some(title));

        let description_el = self
            .document
            .create_element("p")?
            .dyn_into::<HtmlElement>()?;
        description_el.set_class_name("achievement-toast__description");
        description_el.set_text_content(Some(description));

        content.append_child(&title_el)?;
        content.append_child(&description_el)?;
        toast.append_child(&icon)?;
        toast.append_child(&content)?;
        self.achievement_layer.append_child(&toast)?;
        toast.set_attribute("data-state", "visible")?;

        Ok(toast)
    }

    pub fn render_pokemon_capture_attempt(&self) -> Result<HtmlElement, JsValue> {
        self.render_pokemon_effect(
            "pokemon-effect--capture",
            "./effects/capture.gif",
            "The Poké Ball wobbles while attempting to capture Pikachu",
            None,
        )
    }

    pub fn render_pokemon_capture_success(&self) -> Result<HtmlElement, JsValue> {
        self.render_pokemon_effect(
            "pokemon-effect--success",
            "./effects/captured.gif",
            "Pikachu has been captured successfully",
            Some("./effects/captured.mp3"),
        )
    }

    pub fn render_cookie_clicker(&self) -> Result<CookieClickerView, JsValue> {
        let line = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlDivElement>()?;
        line.set_class_name("line output-text cookie-clicker-line");

        let wrapper = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlElement>()?;
        wrapper.set_class_name("cookie-clicker");

        let prompt = self
            .document
            .create_element("p")?
            .dyn_into::<HtmlElement>()?;
        prompt.set_class_name("cookie-clicker__prompt");
        prompt.set_text_content(Some("Click the cookie to start baking the sweetest storm."));

        let button = self
            .document
            .create_element("button")?
            .dyn_into::<HtmlButtonElement>()?;
        button.set_class_name("cookie-clicker__button");
        button.set_type("button");
        button.set_attribute("aria-label", "Click the cookie to bake more points")?;

        let image = self
            .document
            .create_element("img")?
            .dyn_into::<HtmlImageElement>()?;
        image.set_class_name("cookie-clicker__image");
        image.set_src("./effects/cookie.webp");
        image.set_alt("A delicious chocolate chip cookie ready to be clicked");
        image.set_attribute("draggable", "false")?;
        image.set_attribute("loading", "lazy")?;
        let image_node: Node = image.into();
        button.append_child(&image_node)?;

        let counter = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlElement>()?;
        counter.set_class_name("cookie-clicker__counter cookie-clicker__counter--tier0");
        counter.set_text_content(Some("0 / 100"));

        let hint = self
            .document
            .create_element("p")?
            .dyn_into::<HtmlElement>()?;
        hint.set_class_name("cookie-clicker__hint");
        hint.set_text_content(Some("Every tap powers the oven. Keep going!"));

        let prompt_node: Node = prompt.clone().into();
        wrapper.append_child(&prompt_node)?;

        let button_node: Node = button.clone().into();
        wrapper.append_child(&button_node)?;

        let counter_node: Node = counter.clone().into();
        wrapper.append_child(&counter_node)?;

        let hint_node: Node = hint.clone().into();
        wrapper.append_child(&hint_node)?;

        let wrapper_node: Node = wrapper.clone().into();
        line.append_child(&wrapper_node)?;
        self.output.append_child(&line)?;

        let line_element: HtmlElement = line.clone().dyn_into::<HtmlElement>()?;
        self.apply_scroll(&line_element, ScrollBehavior::Bottom)?;

        Ok(CookieClickerView {
            line: line_element,
            wrapper,
            button,
            counter,
            prompt,
        })
    }

    pub fn render_cookie_rain(&self, drops: usize) -> Result<HtmlElement, JsValue> {
        let layer = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlDivElement>()?;
        layer.set_class_name("cookie-rain");
        layer.set_attribute("aria-hidden", "true")?;

        let drops = drops.clamp(12, 96);
        for _ in 0..drops {
            let drop = self
                .document
                .create_element("img")?
                .dyn_into::<HtmlImageElement>()?;
            drop.set_class_name("cookie-rain__drop");
            drop.set_src("./effects/small_cookie.webp");
            drop.set_alt("");
            drop.set_attribute("loading", "lazy")?;
            drop.set_attribute("draggable", "false")?;

            let drop_element: HtmlElement = drop.clone().dyn_into::<HtmlElement>()?;
            let style = drop_element.style();
            let left = format!("{:.2}%", Math::random() * 100.0);
            let scale = 0.78 + (Math::random() * 0.42);
            let duration = 2.1 + (Math::random() * 1.2);
            let delay = Math::random() * 0.9;
            style.set_property("left", &left)?;
            style.set_property("--cookie-scale", &format!("{scale:.3}"))?;
            style.set_property("animation-duration", &format!("{duration:.2}s"))?;
            style.set_property("animation-delay", &format!("{delay:.2}s"))?;

            let drop_node: Node = drop.into();
            layer.append_child(&drop_node)?;
        }

        let layer_element: HtmlElement = layer.clone().dyn_into::<HtmlElement>()?;
        let layer_node: Node = layer.into();
        self.terminal_root.append_child(&layer_node)?;
        Ok(layer_element)
    }

    fn render_pokemon_effect(
        &self,
        modifier: &str,
        image_src: &str,
        image_alt: &str,
        audio_src: Option<&str>,
    ) -> Result<HtmlElement, JsValue> {
        let wrapper = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlDivElement>()?;
        wrapper.set_class_name("line output-text pokemon-effect-line");

        let figure = self
            .document
            .create_element("figure")?
            .dyn_into::<HtmlElement>()?;
        figure.set_class_name(&format!("pokemon-effect {modifier}"));

        let image = self
            .document
            .create_element("img")?
            .dyn_into::<HtmlImageElement>()?;
        image.set_class_name("pokemon-effect__image");
        image.set_src(image_src);
        image.set_alt(image_alt);
        image.set_attribute("loading", "lazy")?;

        let image_node: Node = image.into();
        figure.append_child(&image_node)?;

        if let Some(src) = audio_src {
            let audio = self
                .document
                .create_element("audio")?
                .dyn_into::<HtmlAudioElement>()?;
            audio.set_class_name("pokemon-effect__audio");
            audio.set_src(src);
            audio.set_preload("auto");
            audio.set_autoplay(true);
            let _ = audio.set_attribute("playsinline", "true");
            let audio_node: Node = audio.clone().into();
            figure.append_child(&audio_node)?;

            if let Err(err) = audio.play() {
                utils::log(&format!("Failed to autoplay Pokémon audio: {:?}", err));
            }
        }

        let figure_node: Node = figure.into();
        wrapper.append_child(&figure_node)?;
        self.output.append_child(&wrapper)?;

        let element: HtmlElement = wrapper.clone().dyn_into::<HtmlElement>()?;
        self.apply_scroll(&element, ScrollBehavior::Bottom)?;
        Ok(element)
    }

    pub fn remove_effect(&self, element: &HtmlElement) -> Result<(), JsValue> {
        if let Some(parent) = element.parent_node() {
            let node: Node = element.clone().into();
            parent.remove_child(&node).map(|_| ())
        } else {
            Ok(())
        }
    }

    fn scroll_to_bottom(&self) {
        if let Some(command) = self.last_command.borrow().as_ref() {
            let _ = self.scroll_to_child(command);
        } else {
            let scroll_height = self.output.scroll_height();
            self.output.set_scroll_top(scroll_height);
        }
    }

    fn scroll_to_child(&self, child: &HtmlElement) -> Result<(), JsValue> {
        let offset = child.offset_top();
        self.output.set_scroll_top(offset);
        Ok(())
    }

    fn apply_scroll(&self, element: &HtmlElement, behavior: ScrollBehavior) -> Result<(), JsValue> {
        match behavior {
            ScrollBehavior::None => {}
            ScrollBehavior::Anchor => {
                self.scroll_to_child(element)?;
            }
            ScrollBehavior::Bottom => {
                self.scroll_to_bottom();
            }
        }
        Ok(())
    }

    pub fn apply_ai_mode(&self, active: bool) -> Result<(), JsValue> {
        let mut indicator_text = "AI Mode: Deactivated";
        if active {
            indicator_text = "AI Mode: Activated";
            self.ai_toggle.class_list().add_1("active")?;
            self.terminal_root.class_list().add_1("ai-mode-active")?;
        } else {
            self.ai_toggle.class_list().remove_1("active")?;
            self.ai_toggle.class_list().remove_1("busy")?;
            self.terminal_root.class_list().remove_1("ai-mode-active")?;
        }
        self.ai_toggle
            .set_attribute("aria-pressed", if active { "true" } else { "false" })?;
        self.ai_indicator.set_attribute("aria-busy", "false")?;
        self.set_ai_indicator_text(indicator_text);
        Ok(())
    }

    pub fn set_ai_indicator_text(&self, text: &str) {
        self.ai_indicator.set_text_content(Some(text));
    }

    pub fn set_ai_busy(&self, busy: bool) -> Result<(), JsValue> {
        if busy {
            self.ai_toggle.class_list().add_1("busy")?;
            self.ai_indicator.set_attribute("aria-busy", "true")?;
        } else {
            self.ai_toggle.class_list().remove_1("busy")?;
            self.ai_indicator.set_attribute("aria-busy", "false")?;
        }
        Ok(())
    }

    pub fn show_ai_loader(&self) -> Result<(), JsValue> {
        if self.document.get_element_by_id(AI_LOADER_ID).is_some() {
            return Ok(());
        }

        let wrapper = self
            .document
            .create_element("div")?
            .dyn_into::<HtmlDivElement>()?;
        wrapper.set_id(AI_LOADER_ID);
        wrapper.set_class_name("line ai-loader");

        let spinner = self
            .document
            .create_element("span")?
            .dyn_into::<HtmlElement>()?;
        spinner.set_class_name("ai-loader__spinner");

        let label = self
            .document
            .create_element("span")?
            .dyn_into::<HtmlSpanElement>()?;
        label.set_class_name("ai-loader__label");
        label.set_text_content(Some("Synthesizing answer"));

        let dots = self
            .document
            .create_element("span")?
            .dyn_into::<HtmlSpanElement>()?;
        dots.set_class_name("ai-loader__dots");
        dots.set_text_content(Some("..."));

        wrapper.append_child(&spinner)?;
        wrapper.append_child(&label)?;
        wrapper.append_child(&dots)?;

        self.output.append_child(&wrapper)?;
        self.scroll_to_bottom();
        Ok(())
    }

    pub fn hide_ai_loader(&self) -> Result<(), JsValue> {
        if let Some(node) = self.document.get_element_by_id(AI_LOADER_ID) {
            let node: web_sys::Node = node.unchecked_into();
            let _ = self.output.remove_child(&node)?;
        }
        Ok(())
    }
}

fn get_html_element(document: &Document, id: &str) -> Result<HtmlElement, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("Missing element #{id}")))
        .and_then(|el| {
            el.dyn_into::<HtmlElement>()
                .map_err(|_| JsValue::from_str(&format!("Element #{id} is not HtmlElement")))
        })
}

fn clear_children(element: &HtmlElement) -> Result<(), JsValue> {
    while let Some(child) = element.first_child() {
        element.remove_child(&child)?;
    }
    Ok(())
}
