//! Bounded, sanitized local diagnostics for provider failures.
//!
//! The file intentionally contains no prompts, narration, raw model body, or
//! credentials. It is a small JSONL trail for finding whether a failure was at
//! transport, decode, JSON extraction, or validation.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use story_model::ProviderError;

const MAX_DIAGNOSTIC_BYTES: u64 = 128 * 1024;

#[derive(Serialize)]
struct Event<'a> {
    at: String,
    generation_id: String,
    stage: &'a str,
    category: &'a str,
}

pub fn record_provider_failure(
    save_dir: &Path,
    generation_id: story_model::uuid::Uuid,
    error: &ProviderError,
) {
    let path = save_dir.join("diagnostics.jsonl");
    let category = match error {
        ProviderError::Unavailable(_) => "unavailable",
        ProviderError::Loading(_) => "loading",
        ProviderError::Protocol(_) => "protocol",
        ProviderError::Timeout { .. } => "timeout",
        ProviderError::Cancelled => "cancelled",
        ProviderError::UnsupportedCapability(_) => "unsupported_capability",
        ProviderError::InvalidGeneration(_) => "invalid_generation",
    };
    let event = Event {
        at: story_model::chrono::Utc::now().to_rfc3339(),
        generation_id: generation_id.to_string(),
        stage: super::provider_failure_stage(error),
        category,
    };
    let Ok(line) = serde_json::to_string(&event) else {
        return;
    };
    if fs::create_dir_all(save_dir).is_err() {
        return;
    }
    // Keep this opt-out-free internal aid from growing forever. The records
    // are only for the latest local failures, not an archival event store.
    if fs::metadata(&path)
        .map(|metadata| metadata.len() >= MAX_DIAGNOSTIC_BYTES)
        .unwrap_or(false)
        && fs::write(&path, "").is_err()
    {
        return;
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{line}");
}
