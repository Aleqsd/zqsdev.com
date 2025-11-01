use crate::terminal::{HistoryDirection, Terminal};
use crate::utils;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    ClipboardEvent, CompositionEvent, Element, EventTarget, HtmlElement, KeyboardEvent, MouseEvent,
};

pub fn install_listeners(terminal: Rc<Terminal>) -> Result<(), JsValue> {
    let document = utils::document()?;
    let keydown_terminal = Rc::clone(&terminal);
    let suggestions_terminal = Rc::clone(&terminal);
    let paste_terminal = Rc::clone(&terminal);
    let ai_activation_terminal = Rc::clone(&terminal);
    let composition_terminal = Rc::clone(&terminal);

    let keydown_closure = Closure::wrap(Box::new(move |event: KeyboardEvent| {
        handle_keydown(&keydown_terminal, event);
    }) as Box<dyn FnMut(_)>);

    document
        .add_event_listener_with_callback("keydown", keydown_closure.as_ref().unchecked_ref())?;
    keydown_closure.forget();

    let suggestions = document
        .get_element_by_id("suggestions")
        .ok_or_else(|| JsValue::from_str("Missing #suggestions element"))?
        .dyn_into::<HtmlElement>()?;
    let click_closure = Closure::wrap(Box::new(move |event: MouseEvent| {
        handle_suggestion_click(&suggestions_terminal, event);
    }) as Box<dyn FnMut(_)>);
    suggestions
        .add_event_listener_with_callback("click", click_closure.as_ref().unchecked_ref())?;
    click_closure.forget();

    let paste_closure = Closure::wrap(Box::new(move |event: ClipboardEvent| {
        handle_paste(&paste_terminal, event);
    }) as Box<dyn FnMut(_)>);
    document.add_event_listener_with_callback("paste", paste_closure.as_ref().unchecked_ref())?;
    paste_closure.forget();

    let ai_toggle_terminal = Rc::clone(&terminal);
    let ai_toggle = document
        .get_element_by_id("ai-mode-toggle")
        .ok_or_else(|| JsValue::from_str("Missing #ai-mode-toggle element"))?
        .dyn_into::<HtmlElement>()?;
    let ai_click = Closure::wrap(Box::new(move |event: MouseEvent| {
        event.prevent_default();
        event.stop_propagation();
        if let Err(err) = ai_toggle_terminal.toggle_ai_mode() {
            utils::log(&format!("Failed to toggle AI mode: {:?}", err));
        }
    }) as Box<dyn FnMut(_)>);
    ai_toggle.add_event_listener_with_callback("click", ai_click.as_ref().unchecked_ref())?;
    ai_click.forget();

    let ai_activate_click = Closure::wrap(Box::new(move |event: MouseEvent| {
        if wants_ai_activation(event.target()) {
            event.prevent_default();
            event.stop_propagation();
            if let Err(err) = ai_activation_terminal.activate_ai_mode() {
                utils::log(&format!(
                    "Failed to enable AI mode via suggestion: {:?}",
                    err
                ));
            }
        }
    }) as Box<dyn FnMut(_)>);
    document
        .add_event_listener_with_callback("click", ai_activate_click.as_ref().unchecked_ref())?;
    ai_activate_click.forget();

    let composition_closure = Closure::wrap(Box::new(move |event: CompositionEvent| {
        handle_composition_end(&composition_terminal, event);
    }) as Box<dyn FnMut(_)>);
    document.add_event_listener_with_callback(
        "compositionend",
        composition_closure.as_ref().unchecked_ref(),
    )?;
    composition_closure.forget();

    Ok(())
}

fn handle_keydown(terminal: &Terminal, event: KeyboardEvent) {
    let key = event.key();
    if let Some(command) = lookup_suggestion_command(event.target()) {
        match key.as_str() {
            "Enter" | " " | "Spacebar" => {
                event.prevent_default();
                event.stop_propagation();
                if let Err(err) = terminal.execute_suggestion(&command) {
                    utils::log(&format!(
                        "Error running suggestion `{command}` via keyboard: {:?}",
                        err
                    ));
                }
                return;
            }
            _ => {}
        }
    }

    match key.as_str() {
        "Backspace" => {
            event.prevent_default();
            terminal.delete_last_character();
        }
        "Enter" => {
            event.prevent_default();
            if let Err(err) = terminal.submit_command() {
                utils::log(&format!("Error running command: {:?}", err));
            }
        }
        "Tab" => {
            event.prevent_default();
            terminal.autocomplete();
        }
        "ArrowUp" => {
            event.prevent_default();
            terminal.navigate_history(HistoryDirection::Older);
        }
        "ArrowDown" => {
            event.prevent_default();
            terminal.navigate_history(HistoryDirection::Newer);
        }
        "Escape" => {
            event.prevent_default();
            terminal.clear_input();
        }
        _ => {
            handle_printable(terminal, &event);
        }
    }
}

fn handle_printable(terminal: &Terminal, event: &KeyboardEvent) {
    if event.ctrl_key() || event.meta_key() || event.alt_key() || event.is_composing() {
        return;
    }

    let key = event.key();
    if is_printable_character_key(&key) {
        event.prevent_default();
        terminal.append_character(&key);
    }
}

fn handle_suggestion_click(terminal: &Terminal, event: MouseEvent) {
    if let Some(command) = lookup_suggestion_command(event.target()) {
        event.prevent_default();
        event.stop_propagation();
        if let Err(err) = terminal.execute_suggestion(&command) {
            utils::log(&format!(
                "Error running suggestion `{command}` via mouse: {:?}",
                err
            ));
        }
    }
}

fn lookup_suggestion_command(target: Option<EventTarget>) -> Option<String> {
    let mut current = target.and_then(|value| value.dyn_into::<Element>().ok());
    while let Some(element) = current {
        if element.class_list().contains("suggestion") {
            return element
                .get_attribute("data-command")
                .or_else(|| element.text_content())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
        }
        current = element.parent_element();
    }
    None
}

fn handle_paste(terminal: &Terminal, event: ClipboardEvent) {
    if let Some(data) = event.clipboard_data() {
        if let Ok(raw) = data.get_data("text") {
            let sanitized = sanitize_pasted_text(&raw);
            if !sanitized.is_empty() {
                event.prevent_default();
                terminal.append_text(&sanitized);
            }
        }
    }
}

fn sanitize_pasted_text(input: &str) -> String {
    let mut sanitized = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\r' => {}
            '\n' | '\t' => sanitized.push(' '),
            _ => sanitized.push(ch),
        }
    }
    sanitized.trim_matches(' ').to_string()
}

fn is_printable_character_key(key: &str) -> bool {
    if matches!(key, "Dead" | "Process") {
        return false;
    }

    let mut chars = key.chars();
    matches!((chars.next(), chars.next()), (Some(_), None))
}

fn handle_composition_end(terminal: &Terminal, event: CompositionEvent) {
    if let Some(data) = event.data() {
        if data.is_empty() {
            return;
        }
        event.prevent_default();
        terminal.append_text(&data);
    }
}

fn wants_ai_activation(target: Option<EventTarget>) -> bool {
    let mut current = target.and_then(|value| value.dyn_into::<Element>().ok());
    while let Some(element) = current {
        if let Some(action) = element.get_attribute("data-action") {
            if action.eq_ignore_ascii_case("activate-ai-mode") {
                return true;
            }
        }
        current = element.parent_element();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{is_printable_character_key, sanitize_pasted_text};

    #[test]
    fn sanitize_trims_and_flattens_whitespace() {
        let raw = " hello\tworld \nsecond line\r\n";
        let cleaned = sanitize_pasted_text(raw);
        assert_eq!(cleaned, "hello world second line");
    }

    #[test]
    fn sanitize_preserves_internal_spacing() {
        let raw = "keep  spacing";
        let cleaned = sanitize_pasted_text(raw);
        assert_eq!(cleaned, "keep  spacing");
    }

    #[test]
    fn printable_key_detects_single_unicode_scalar() {
        assert!(is_printable_character_key("a"));
        assert!(is_printable_character_key(" "));
        assert!(is_printable_character_key("é"));
        assert!(is_printable_character_key("ç"));
        assert!(is_printable_character_key("京"));
    }

    #[test]
    fn printable_key_rejects_control_sequences() {
        assert!(!is_printable_character_key(""));
        assert!(!is_printable_character_key("Enter"));
        assert!(!is_printable_character_key("ArrowLeft"));
        assert!(!is_printable_character_key("Dead"));
        assert!(!is_printable_character_key("Process"));
    }
}
