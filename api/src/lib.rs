//! zqsdev-api: the Cloudflare Worker (workers-rs) that serves `/api/*` for zqsdev.com.
//!
//! `core` is plain Rust with no `worker` types: request handling, the provider call protocol, the
//! spending ledger (`core::ledger::Book`) and every SQL statement live there and are tested natively
//! (`cargo test --lib`, real SQLite through rusqlite). The wasm32-only glue adapts them to the
//! Workers runtime: fetch handler, Durable Object, `transactionSync`, `fetch` and `crypto`.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::todo,
        clippy::unimplemented
    )
)]

pub mod core;
#[cfg(target_arch = "wasm32")]
mod glue;
