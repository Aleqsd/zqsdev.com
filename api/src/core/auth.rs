//! Admin token check.

/// `ADMIN_TOKEN` shorter than this disables every `/api/admin/*` route.
pub const MIN_ADMIN_TOKEN_CHARS: usize = 32;
/// Start of the public placeholder that `api/.dev.vars.example` once shipped: never a secret.
pub const PLACEHOLDER_PREFIX: &str = "replace-with";

/// The configured admin secret when it can enable the admin routes: long enough and not a
/// documented placeholder.
pub fn usable_token(secret: Option<String>) -> Option<String> {
    secret.filter(|t| {
        t.chars().count() >= MIN_ADMIN_TOKEN_CHARS && !t.starts_with(PLACEHOLDER_PREFIX)
    })
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
        assert_eq!(usable_token(Some(String::new())), None);
        // The 68-character placeholder of earlier `.dev.vars.example` files is public.
        let placeholder = "replace-with-python3-c-import-secrets-print-secrets-token_urlsafe-32";
        assert!(placeholder.len() >= MIN_ADMIN_TOKEN_CHARS);
        assert_eq!(usable_token(Some(placeholder.to_string())), None);
    }
}
