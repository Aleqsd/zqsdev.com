//! Pure core of the API: no `worker` import, compiled and tested natively.

pub mod api;
pub mod auth;
pub mod client_ip;
pub mod config;
pub mod embedded;
pub mod http;
pub mod ledger;
pub mod logline;
pub mod money;
pub mod platform;
pub mod prompt;
pub mod provider;
pub mod reply;
pub mod request;
pub mod sanitize;
pub mod time;

#[cfg(test)]
pub mod testkit;
