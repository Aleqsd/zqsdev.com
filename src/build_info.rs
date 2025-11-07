pub const FRONTEND_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn frontend_commit() -> &'static str {
    option_env!("GIT_COMMIT_HASH").unwrap_or("unknown")
}
