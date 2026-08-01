//! StoryTUI binary entrypoint.
//!
//! W0–W11 land the engine, providers, TUI screens, persistence, and the
//! diagnostic / benchmark library surfaces. The interactive host loop (event
//! pump + provider polling) is documented as an open runtime item — see
//! `docs/KNOWN-LIMITATIONS.md` and `docs/RELEASE-RUNTIME-GATE.md`.

fn main() {
    println!(
        "storytui {} — host loop not yet wired; see docs/RELEASE-RUNTIME-GATE.md",
        env!("CARGO_PKG_VERSION")
    );
}
