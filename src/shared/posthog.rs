use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

const EU_HOST: &str = "https://eu.i.posthog.com";

static ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
struct TenjinHttpError {
    message: String,
}

impl std::fmt::Display for TenjinHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TenjinHttpError {}

/// Initialize the process-global PostHog client. Returns whether capture is on.
pub async fn init() -> bool {
    let key = std::env::var("POSTHOG_PROJECT_API_KEY")
        .or_else(|_| std::env::var("POSTHOG_PROJECT_TOKEN"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let Some(key) = key else {
        return false;
    };

    let host = std::env::var("POSTHOG_HOST")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| EU_HOST.to_string());

    let error_tracking = match posthog_rs::ErrorTrackingOptionsBuilder::default()
        .capture_panics(true)
        .build()
    {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("posthog error-tracking options failed: {err}");
            return false;
        }
    };

    let options = match posthog_rs::ClientOptionsBuilder::default()
        .api_key(key)
        .host(host)
        .error_tracking(error_tracking)
        .build()
    {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("posthog client options failed: {err}");
            return false;
        }
    };

    if let Err(err) = posthog_rs::init_global(options).await {
        eprintln!("posthog init_global failed: {err}");
        return false;
    }

    ENABLED.store(true, Ordering::Relaxed);
    true
}

fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn skip_path(path: &str) -> bool {
    path == "/" || path == "/health"
}

pub async fn capture_server_errors(request: Request, next: Next) -> Response {
    if !is_enabled() {
        return next.run(request).await;
    }

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let distinct_id = request
        .headers()
        .get("x-posthog-distinct-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let skip = skip_path(&path);
    let response = next.run(request).await;
    if !skip && response.status().is_server_error() {
        let status = response.status();
        tokio::spawn(async move {
            report_server_error(method.as_str(), &path, status, distinct_id.as_deref()).await;
        });
    }
    response
}

async fn report_server_error(
    method: &str,
    path: &str,
    status: StatusCode,
    distinct_id: Option<&str>,
) {
    let error = TenjinHttpError {
        message: format!("{method} {path} returned {status}"),
    };
    let mut options = posthog_rs::CaptureExceptionOptions::new();
    if let Some(id) = distinct_id {
        options = options.distinct_id(id);
    }
    let options = options
        .property("service", "tenjin")
        .and_then(|opts| opts.property("route", path))
        .and_then(|opts| opts.property("http_method", method))
        .and_then(|opts| opts.property("status", status.as_u16()))
        .unwrap_or_else(|_| posthog_rs::CaptureExceptionOptions::new());
    if let Err(err) = posthog_rs::capture_exception_with(&error, options).await {
        eprintln!("posthog capture_exception failed: {err}");
    }
}
