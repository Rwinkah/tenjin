//! Service-to-service auth for permit routes.
//!
//! Join, leave and create permits let a wallet move funds on-chain, so only wager-backend may
//! request them: it checks the user's passkey first, then calls Tenjin with the shared
//! `TENJIN_SERVICE_KEY`. Settlement attestations stay public (they only attest game results).

use std::sync::OnceLock;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub const SERVICE_KEY_HEADER: &str = "x-tenjin-service-key";

static SERVICE_KEY: OnceLock<Option<String>> = OnceLock::new();

fn configured_key() -> Option<&'static str> {
    SERVICE_KEY
        .get_or_init(|| {
            std::env::var("TENJIN_SERVICE_KEY")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .as_deref()
}

/// Compare without short-circuiting on the first differing byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Rejects the request unless it carries the configured service key.
/// Fails closed: with no key configured, permit routes are unavailable.
pub async fn require_service_key(request: Request, next: Next) -> Response {
    let Some(expected) = configured_key() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "TENJIN_SERVICE_KEY is not configured",
        )
            .into_response();
    };

    let provided = request
        .headers()
        .get(SERVICE_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or("");

    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "permit requests require a service key").into_response();
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn compares_equal_and_unequal_keys() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret-longer"));
        assert!(!constant_time_eq(b"", b"secret"));
    }
}
