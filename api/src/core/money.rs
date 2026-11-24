//! Exact money arithmetic: token rates in pico-USD per token, amounts in nano-USD (`i64`).
//!
//! The original server computed in `f64` USD; the formulas are the same, evaluated on integers so
//! that window sums are exact and a reservation is never above its exact value.

use serde_json::Value;

use super::provider::field;

/// One reservation or settlement above $1,000,000 is refused.
pub const MAX_NUSD: i64 = 1_000_000_000_000_000;
/// Budget limits above $1e9 are clamped to this ceiling.
pub const LIMIT_CEILING_NUSD: i64 = 1_000_000_000_000_000_000;
/// Prices above $1,000,000 per million tokens are invalid configuration.
pub const MAX_PRICE_USD_PER_MILLION: f64 = 1_000_000.0;
/// Bytes added to the body length to bound prompt tokens conservatively (original `reservation`).
pub const PROMPT_FRAMING_BYTES: u128 = 512;
/// `max_completion_tokens` of every provider request.
pub const MAX_OUTPUT_TOKENS: u128 = 640;

/// Token prices in pico-USD per token (USD per million tokens × 1e6), each in `0..=1e12`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rates {
    pub input: i64,
    pub cached: i64,
    pub write: i64,
    pub output: i64,
}

impl Rates {
    /// Rates from USD per million tokens `[input, cached, cache_write, output]`; `None` when one
    /// is not finite, negative or above $1,000,000 per million.
    pub fn from_usd_per_million(prices: [f64; 4]) -> Option<Rates> {
        let [input, cached, write, output] = prices;
        Some(Rates {
            input: rate_pico(input)?,
            cached: rate_pico(cached)?,
            write: rate_pico(write)?,
            output: rate_pico(output)?,
        })
    }
}

/// `round(p × 1e6)` pico-USD per token for a price `p` in USD per million tokens.
pub fn rate_pico(usd_per_million: f64) -> Option<i64> {
    // Also refuses NaN and infinities.
    if !(0.0..=MAX_PRICE_USD_PER_MILLION).contains(&usd_per_million) {
        return None;
    }
    // ≤ 1e12, exactly representable; the cast cannot saturate.
    Some((usd_per_million * 1e6).round() as i64)
}

/// Reservation for a provider body of `body_len` bytes:
/// `ceil(((L + 512) × max(input, cached, write) + 640 × output) / 1000)` nano-USD, saturating.
pub fn reserve_nusd(body_len: usize, rates: &Rates) -> i64 {
    let max_input = u128::try_from(rates.input.max(rates.cached).max(rates.write)).unwrap_or(0);
    let output = u128::try_from(rates.output).unwrap_or(0);
    let length = u128::try_from(body_len)
        .unwrap_or(u128::MAX)
        .saturating_add(PROMPT_FRAMING_BYTES);
    let pico = length
        .saturating_mul(max_input)
        .saturating_add(MAX_OUTPUT_TOKENS.saturating_mul(output));
    i64::try_from(pico.div_ceil(1000)).unwrap_or(i64::MAX)
}

/// Token counts of a provider `usage` object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsageTokens {
    pub prompt: u64,
    pub completion: u64,
    pub cached: u64,
    pub writes: u64,
}

/// Why a provider `usage` object cannot be priced (messages of the original `usage_cost`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageError {
    MissingInput,
    MissingOutput,
    InvalidCache,
}

impl UsageError {
    pub fn message(self) -> &'static str {
        match self {
            UsageError::MissingInput => "Missing input usage",
            UsageError::MissingOutput => "Missing output usage",
            UsageError::InvalidCache => "Invalid cache usage",
        }
    }

    /// Error class stored in the ledger.
    pub fn class(self) -> &'static str {
        match self {
            UsageError::MissingInput | UsageError::MissingOutput => "missing_usage",
            UsageError::InvalidCache => "invalid_cache_usage",
        }
    }
}

/// The checks of the original `usage_cost`: `prompt_tokens` and `completion_tokens` required,
/// `prompt_tokens_details.cached_tokens` and `.cache_write_tokens` default to 0 and their sum
/// must not exceed `prompt_tokens`. Reasoning tokens are part of `completion_tokens`.
pub fn usage_tokens(usage: &Value) -> Result<UsageTokens, UsageError> {
    let prompt = field(usage, "prompt_tokens")
        .as_u64()
        .ok_or(UsageError::MissingInput)?;
    let completion = field(usage, "completion_tokens")
        .as_u64()
        .ok_or(UsageError::MissingOutput)?;
    let details = field(usage, "prompt_tokens_details");
    let cached = field(details, "cached_tokens").as_u64().unwrap_or(0);
    let writes = field(details, "cache_write_tokens").as_u64().unwrap_or(0);
    match cached.checked_add(writes) {
        Some(sum) if sum <= prompt => Ok(UsageTokens {
            prompt,
            completion,
            cached,
            writes,
        }),
        _ => Err(UsageError::InvalidCache),
    }
}

/// Actual cost: `floor((uncached × input + cached × cached_rate + writes × write_rate
/// + completion × output + 500) / 1000)` nano-USD (half-up rounding), clamped to `MAX_NUSD`.
pub fn usage_nusd(tokens: &UsageTokens, rates: &Rates) -> i64 {
    let rate = |r: i64| u128::try_from(r).unwrap_or(0);
    let uncached = tokens
        .prompt
        .saturating_sub(tokens.cached)
        .saturating_sub(tokens.writes);
    let pico = u128::from(uncached)
        .saturating_mul(rate(rates.input))
        .saturating_add(u128::from(tokens.cached).saturating_mul(rate(rates.cached)))
        .saturating_add(u128::from(tokens.writes).saturating_mul(rate(rates.write)))
        .saturating_add(u128::from(tokens.completion).saturating_mul(rate(rates.output)))
        .saturating_add(500);
    i64::try_from(pico / 1000).map_or(MAX_NUSD, |n| n.min(MAX_NUSD))
}

/// A budget in USD as a nano-USD limit: `min(round(x × 1e9), 1e18)`; `None` when the value is
/// negative or not finite (every admission is then denied with `budget_unavailable`).
pub fn limit_nusd(usd: f64) -> Option<i64> {
    if !usd.is_finite() || usd < 0.0 {
        return None;
    }
    let nano = (usd * 1e9).round();
    if nano >= 1e18 {
        Some(LIMIT_CEILING_NUSD)
    } else {
        Some(nano as i64)
    }
}

/// Nano-USD as an `f64` USD amount (exports and logs: serde_json's shortest form).
pub fn usd(nusd: i64) -> f64 {
    nusd as f64 / 1e9
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn luna56() -> Rates {
        Rates::from_usd_per_million([0.20, 0.02, 0.25, 1.20]).unwrap()
    }

    fn luna6() -> Rates {
        Rates::from_usd_per_million([0.10, 0.01, 0.125, 0.50]).unwrap()
    }

    /// Ported from `ai.rs`: the gpt-5.6-luna rates are pinned; $0.000373 = 373,000 nano-USD.
    #[test]
    fn counts_cached_write_output_and_reasoning_usage_once() {
        let u = json!({"prompt_tokens":1000,"completion_tokens":200,"prompt_tokens_details":{"cached_tokens":400,"cache_write_tokens":100},"completion_tokens_details":{"reasoning_tokens":50}});
        assert_eq!(usage_nusd(&usage_tokens(&u).unwrap(), &luna56()), 373_000);
        assert!(usage_tokens(&json!({})).is_err());
    }

    #[test]
    fn rate_conversion_and_rounding() {
        assert_eq!(
            luna6(),
            Rates {
                input: 100_000,
                cached: 10_000,
                write: 125_000,
                output: 500_000
            }
        );
        assert_eq!(rate_pico(0.1), Some(100_000));
        assert_eq!(rate_pico(1e6), Some(1_000_000_000_000));
        assert_eq!(rate_pico(0.000_000_4), Some(0));
        assert_eq!(rate_pico(0.000_000_6), Some(1));
        assert_eq!(rate_pico(1e6 + 1.0), None);
        assert_eq!(rate_pico(-0.01), None);
        assert_eq!(rate_pico(f64::NAN), None);
        assert_eq!(rate_pico(f64::INFINITY), None);
        assert_eq!(rate_pico(-0.0), Some(0));
    }

    #[test]
    fn reservation_vectors() {
        // "Micro Mages?" with gpt-6-luna: 17,184 bytes.
        assert_eq!(reserve_nusd(17_184, &luna6()), 2_532_000);
        // Same formula as the f64 original, exact: (L + 512) × 0.25 + 640 × 1.2 per million.
        assert_eq!(reserve_nusd(17_186, &luna56()), 5_192_500);
        // ceil: one pico-USD above a nano-USD boundary rounds up.
        let odd = Rates {
            input: 1,
            cached: 0,
            write: 0,
            output: 0,
        };
        assert_eq!(reserve_nusd(488, &odd), 1);
        assert_eq!(reserve_nusd(489, &odd), 2);
        assert_eq!(reserve_nusd(0, &Rates::default()), 0);
        // Saturates instead of wrapping.
        let huge = Rates {
            input: 1_000_000_000_000,
            cached: 0,
            write: 0,
            output: 1_000_000_000_000,
        };
        assert_eq!(reserve_nusd(usize::MAX, &huge), i64::MAX);
        assert!(reserve_nusd(82_000, &huge) < MAX_NUSD);
        assert!(reserve_nusd(2_000_000, &huge) > MAX_NUSD);
    }

    #[test]
    fn usage_rounds_half_up_and_clamps() {
        let r = Rates {
            input: 1,
            cached: 0,
            write: 0,
            output: 0,
        };
        let t = |prompt| UsageTokens {
            prompt,
            completion: 0,
            cached: 0,
            writes: 0,
        };
        assert_eq!(usage_nusd(&t(499), &r), 0);
        assert_eq!(usage_nusd(&t(500), &r), 1);
        assert_eq!(usage_nusd(&t(1_499), &r), 1);
        assert_eq!(usage_nusd(&t(1_500), &r), 2);
        let huge = Rates {
            input: 1_000_000_000_000,
            cached: 0,
            write: 0,
            output: 0,
        };
        assert_eq!(usage_nusd(&t(u64::MAX), &huge), MAX_NUSD);
        // gpt-5.6-luna, 100 in / 20 out: (100 × 0.20 + 20 × 1.20) / 1e6 USD.
        let u = usage_tokens(&json!({"prompt_tokens":100,"completion_tokens":20})).unwrap();
        assert_eq!(usage_nusd(&u, &luna56()), 44_000);
        assert_eq!(usage_nusd(&u, &luna6()), 20_000);
    }

    #[test]
    fn usage_errors_in_original_order() {
        assert_eq!(
            usage_tokens(&json!({"completion_tokens":1})),
            Err(UsageError::MissingInput)
        );
        assert_eq!(
            usage_tokens(&json!({"prompt_tokens":1})),
            Err(UsageError::MissingOutput)
        );
        assert_eq!(
            usage_tokens(&json!({"prompt_tokens":1.5,"completion_tokens":1})),
            Err(UsageError::MissingInput)
        );
        assert_eq!(
            usage_tokens(
                &json!({"prompt_tokens":10,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":6,"cache_write_tokens":5}})
            ),
            Err(UsageError::InvalidCache)
        );
        assert_eq!(
            usage_tokens(
                &json!({"prompt_tokens":10,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":u64::MAX,"cache_write_tokens":u64::MAX}})
            ),
            Err(UsageError::InvalidCache)
        );
        assert_eq!(UsageError::MissingOutput.message(), "Missing output usage");
        assert_eq!(UsageError::InvalidCache.class(), "invalid_cache_usage");
    }

    #[test]
    fn limits_clamp_and_checked_sums_deny_on_overflow() {
        assert_eq!(limit_nusd(0.5), Some(500_000_000));
        assert_eq!(limit_nusd(10.0), Some(10_000_000_000));
        assert_eq!(limit_nusd(0.0), Some(0));
        assert_eq!(limit_nusd(1e9), Some(LIMIT_CEILING_NUSD));
        assert_eq!(limit_nusd(1e300), Some(LIMIT_CEILING_NUSD));
        assert_eq!(limit_nusd(-1.0), None);
        assert_eq!(limit_nusd(f64::NAN), None);
        assert_eq!(limit_nusd(f64::INFINITY), None);
        let sum: i64 = i64::MAX - 5;
        let deny = |nusd: i64, limit: i64| sum.checked_add(nusd).is_none_or(|s| s > limit);
        assert!(deny(10, i64::MAX));
        assert!(!deny(5, i64::MAX));
    }

    #[test]
    fn usd_uses_shortest_form() {
        assert_eq!(serde_json::to_string(&usd(873_000)).unwrap(), "0.000873");
        assert_eq!(serde_json::to_string(&usd(2_532_000)).unwrap(), "0.002532");
        assert_eq!(serde_json::to_string(&usd(20_000_000)).unwrap(), "0.02");
        assert_eq!(serde_json::to_string(&usd(5_000)).unwrap(), "5e-6");
    }
}
