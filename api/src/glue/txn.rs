//! `ctx.storage.transactionSync(callback)`, which workers-rs 0.9.0 does not wrap. An `Err` from
//! the work becomes a JavaScript throw inside the callback, so SQLite rolls back every statement
//! it ran; the typed error is handed back to the caller through a slot.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use worker::js_sys::{self, Function, Reflect};
use worker::wasm_bindgen::closure::Closure;
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::Storage;

use super::log::js_error;

/// Why a transaction did not commit. Either way nothing it wrote remains.
pub enum TxnError<E> {
    /// The work failed (and was rolled back).
    Work(E),
    /// `transactionSync` failed by itself: missing, or the commit threw.
    Js(String),
}

/// Runs `work` inside one SQLite transaction of the Durable Object's storage. `work` must not
/// await: the callback is synchronous, which also keeps other events out (input gate).
pub fn transaction_sync<T, E>(
    storage: &Storage,
    work: impl FnOnce() -> Result<T, E> + 'static,
) -> Result<T, TxnError<E>>
where
    T: 'static,
    E: fmt::Display + 'static,
{
    let slot: Rc<RefCell<Option<Result<T, E>>>> = Rc::new(RefCell::new(None));
    let out = Rc::clone(&slot);
    let callback = Closure::once_into_js(move || -> Result<JsValue, JsValue> {
        let result = work();
        let thrown = result
            .as_ref()
            .err()
            .map(|error| JsValue::from(js_sys::Error::new(&error.to_string())));
        if let Ok(mut slot) = out.try_borrow_mut() {
            *slot = Some(result);
        }
        match thrown {
            None => Ok(JsValue::UNDEFINED),
            // The throw is what rolls the transaction back.
            Some(error) => Err(error),
        }
    });
    let raw: &JsValue = storage.as_raw().as_ref();
    let function = Reflect::get(raw, &JsValue::from_str("transactionSync"))
        .map_err(|error| TxnError::Js(js_error(&error)))?
        .dyn_into::<Function>()
        .map_err(|_| TxnError::Js("storage.transactionSync is not available".into()))?;
    let called = function.call1(raw, &callback);
    let result = slot.try_borrow_mut().ok().and_then(|mut slot| slot.take());
    match (called, result) {
        (_, Some(Err(error))) => Err(TxnError::Work(error)),
        (Err(error), _) => Err(TxnError::Js(js_error(&error))),
        (Ok(_), Some(Ok(value))) => Ok(value),
        (Ok(_), None) => Err(TxnError::Js(
            "transactionSync did not run its callback".into(),
        )),
    }
}
