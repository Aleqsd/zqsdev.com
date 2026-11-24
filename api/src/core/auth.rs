//! Admin token check.

/// `ADMIN_TOKEN` shorter than this disables every `/api/admin/*` route.
pub const MIN_ADMIN_TOKEN_CHARS: usize = 32;

/// The configured admin secret when it is long enough to enable the admin routes.
pub fn usable_token(secret: Option<String>) -> Option<String> {
    secret.filter(|t| t.chars().count() >= MIN_ADMIN_TOKEN_CHARS)
}

/// Compares the expected token with a candidate in time that depends only on their lengths.
pub fn token_matches(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    let mut diff = u8::from(a.len() != b.len());
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_rules() {
        let token = "t".repeat(43);
        assert!(token_matches(&token, &token));
        assert!(!token_matches(&token, &"t".repeat(42)));
        assert!(!token_matches(&token, &format!("{token}x")));
        assert!(!token_matches(&token, ""));
        assert!(!token_matches(&token, &format!("u{}", "t".repeat(42))));
        assert_eq!(usable_token(Some("x".repeat(31))), None);
        assert_eq!(usable_token(Some("x".repeat(32))), Some("x".repeat(32)));
        assert_eq!(usable_token(None), None);
    }
}
