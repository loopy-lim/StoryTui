//! OpenAI-compatible request/response envelope — STUB (ST-016 scaffolding).
//!
//! The real envelope types (chat-completions request body, streaming SSE
//! frames, `response_format` JSON Schema wiring) live here in W6, consumed by
//! the real HTTP provider. They are deliberately NOT implemented in Tranche 1:
//! `MockProvider` bypasses the envelope entirely and returns
//! `GeneratedTurnDraft`s directly, so this module stays a placeholder to keep
//! the file layout promised by the plan in place without introducing a real
//! network client.
//!
//! When W6 lands, replace the contents of this module with the concrete
//! request/response structs and a `reqwest`-based sender. No code outside this
//! module should change shape.
