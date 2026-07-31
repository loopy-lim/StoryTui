#![warn(clippy::all)]
//! LLM provider abstraction for StoryTUI: the trait surface and a local MLX
//! HTTP client implementation. Owns all network interaction (`reqwest`) so that
//! [`story-core`] and [`story-model`] remain IO-free. Streaming, retry, and
//! prompt scaffolding live here.
