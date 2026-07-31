//! [`OpenAiCompatibleProvider`] — the real local-LLM HTTP adapter (ST-017).
//!
//! Talks to an OpenAI-compatible server (MLX-VLM / MLX-LM) at `127.0.0.1`.
//! Builds the chat-completions request envelope from a [`TurnRequest`], POSTs
//! to `/v1/chat/completions`, extracts the assistant message body, and runs it
//! through the existing [`recovery::parse_generated_turn`]. ADR-002: loopback
//! default; non-loopback warned (NOT blocked); no redirects; no telemetry.
//!
//! W6.3 flesh-out lands here; W6.1 only needs the loopback predicate so
//! `config` tests can reference it.

/// Returns `true` when the host part of `url` is a loopback address
/// (`127.0.0.0/8`, `localhost`, `::1`). Used to decide the ADR-002 non-loopback
/// warning — no external URL crate is pulled in (ADR-003: keep deps minimal).
pub fn is_loopback_base_url(url: &str) -> bool {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    // Pull the host: bracketed IPv6 (`[::1]:port`) vs. the more common
    // `host:port` / `host/path` shapes.
    let host = if after_scheme.starts_with('[') {
        let end = after_scheme.find(']').unwrap_or(after_scheme.len());
        &after_scheme[..end]
    } else {
        let host_end = after_scheme.find([':', '/']).unwrap_or(after_scheme.len());
        &after_scheme[..host_end]
    };
    is_loopback_host(host)
}

fn is_loopback_host(host: &str) -> bool {
    let h = host.trim();
    h == "127.0.0.1" || h == "localhost" || h == "::1" || h == "[::1]" || h.starts_with("127.")
}

/// Placeholder; the real `reqwest`-backed implementation lands in W6.3.
#[derive(Debug)]
pub struct OpenAiCompatibleProvider;
