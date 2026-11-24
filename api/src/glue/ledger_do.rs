//! The `Ledger` Durable Object (SQLite-backed, one instance `zqs-ledger-v1`). Each request is
//! one op: the body is read first, then `core::ledger::serve` runs it synchronously on the cached
//! `Book` inside `transactionSync`. No fetch, no timer, no alarm.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde_json::{json, Value};
// `#[durable_object]` expands to `#[wasm_bindgen]` items that name this path.
use worker::wasm_bindgen;
use worker::{
    durable_object, Date, DurableObject, Env, Request, Response, ResponseBuilder, SqlStorage,
    State, Storage,
};

use super::config::string_binding;
use super::log;
use super::sql_store::{create_schema, SqlStore};
use super::txn::{transaction_sync, TxnError};
use crate::core::config::LedgerConfig;
use crate::core::http::APPLICATION_JSON;
use crate::core::ledger::serve::{serve, DoLog, DoResponse, LedgerHost};
use crate::core::ledger::{Book, LedgerError};
use crate::core::logline::{Level, Throttle};

#[durable_object(fetch)]
pub struct Ledger {
    storage: Storage,
    sql: SqlStorage,
    /// The in-memory book, loaded by the first op after a wake and dropped after any failed
    /// transaction. Only borrowed inside a synchronous transaction, never across an await.
    book: Rc<RefCell<Option<Book>>>,
    /// Vars cannot change while the object lives (a deploy restarts it).
    cfg: LedgerConfig,
    throttle: Throttle,
    schema_ready: Cell<bool>,
}

impl Ledger {
    /// Creates the schema if needed; logs and reports a failure (the next request retries).
    fn ensure_schema(&self) -> bool {
        if self.schema_ready.get() {
            return true;
        }
        match create_schema(&self.sql) {
            Ok(()) => {
                self.schema_ready.set(true);
                true
            }
            Err(error) => {
                log::emit(
                    self.cfg.log_level,
                    Level::Error,
                    "🔥 Unexpected internal error",
                    &json!({ "route": "ledger", "error": format!("schema: {error}") }),
                );
                false
            }
        }
    }
}

impl DurableObject for Ledger {
    fn new(state: State, env: Env) -> Self {
        let storage = state.storage();
        let sql = storage.sql();
        let cfg = LedgerConfig::from_lookup(&|name| string_binding(&env, name));
        let ledger = Ledger {
            storage,
            sql,
            book: Rc::new(RefCell::new(None)),
            cfg,
            throttle: Throttle::new(),
            schema_ready: Cell::new(false),
        };
        ledger.ensure_schema();
        ledger
    }

    async fn fetch(&self, mut req: Request) -> worker::Result<Response> {
        // The whole op is read before the book is touched.
        let body = req.bytes().await.unwrap_or_default();
        let now_ms = Date::now().as_millis();
        let reply = if self.ensure_schema() {
            let host = DoHost {
                storage: &self.storage,
                sql: &self.sql,
                book: &self.book,
                #[cfg(zqs_fault_injection)]
                fail_after: Cell::new(
                    req.headers()
                        .get(super::platform::FAIL_AFTER_HEADER)
                        .ok()
                        .flatten()
                        .and_then(|value| value.trim().parse::<u32>().ok()),
                ),
            };
            let level = self.cfg.log_level;
            let sink = move |line_level: Level, msg: &'static str, fields: Value| {
                log::emit(level, line_level, msg, &fields);
            };
            serve(
                &host,
                self.cfg,
                now_ms,
                &body,
                &DoLog {
                    sink: &sink,
                    throttle: &self.throttle,
                    now_ms,
                },
            )
        } else {
            DoResponse {
                content_type: APPLICATION_JSON,
                next_after: None,
                body: br#"{"status":"error","error":"storage"}"#.to_vec(),
            }
        };
        let mut builder = ResponseBuilder::new()
            .with_status(200)
            .with_header("Content-Type", reply.content_type)?;
        if let Some(next) = &reply.next_after {
            builder = builder.with_header("X-Next-After", next)?;
        }
        Ok(builder.fixed(reply.body))
    }
}

/// The `LedgerHost` of one request: the object's storage and its cached book.
struct DoHost<'a> {
    storage: &'a Storage,
    sql: &'a SqlStorage,
    book: &'a Rc<RefCell<Option<Book>>>,
    /// Local fault injection: armed for the request's first transaction only.
    #[cfg(zqs_fault_injection)]
    fail_after: Cell<Option<u32>>,
}

impl LedgerHost for DoHost<'_> {
    type Store = SqlStore;

    fn transact<T: 'static>(
        &self,
        now: i64,
        work: impl FnOnce(&mut Book, &mut SqlStore) -> Result<T, LedgerError> + 'static,
    ) -> Result<T, LedgerError> {
        let cell = Rc::clone(self.book);
        #[allow(unused_mut)]
        let mut store = SqlStore::new(self.sql.clone());
        #[cfg(zqs_fault_injection)]
        store.fail_after(self.fail_after.take());
        let outcome = transaction_sync(self.storage, move || -> Result<T, LedgerError> {
            let mut slot = cell
                .try_borrow_mut()
                .map_err(|_| LedgerError::Store("ledger cache already borrowed".into()))?;
            // Taken out: on any error below the cell stays empty and the next op reloads.
            let mut book = match slot.take() {
                Some(book) => book,
                None => Book::load(&mut store, now)?,
            };
            let value = work(&mut book, &mut store)?;
            *slot = Some(book);
            Ok(value)
        });
        outcome.map_err(|error| {
            // Rolled back: the cached book may hold changes that storage no longer has.
            if let Ok(mut slot) = self.book.try_borrow_mut() {
                *slot = None;
            }
            match error {
                TxnError::Work(error) => error,
                TxnError::Js(message) => LedgerError::Store(message),
            }
        })
    }
}
