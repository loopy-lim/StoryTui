//! Character-card importer: compile SillyTavern/RisuAI cards (V1/V2/V3 PNG
//! or JSON, RisuAI `.charx` zip containers) plus Character.AI community-dump
//! JSON and optional World Info into a native StoryTui pack.
//!
//! Policy contract (see `docs/plans/` for the full discussion):
//! - **The parser is total** — the untouched card JSON always ships in
//!   `sources/card.json`; anything not carried into the pack is recorded in
//!   `import-report.md`, never silently dropped.
//! - **Execution is tiered** — lorebook features the engine does not execute
//!   yet (recursion, timers, vectorized search) are preserved and reported.
//! - **The emit is verified** — every import finishes with a
//!   [`story_persistence::PackLoader`] self-check, so the importer and the
//!   loader cannot drift apart.

pub mod card;
pub mod charx;
pub mod chat;
pub mod macros;
pub mod normalize;
pub mod report;
pub mod scaffold;
pub mod worldinfo;

use std::path::{Path, PathBuf};

use report::ImportReport;
use worldinfo::NormalizedBook;

pub use scaffold::{ImportOptions, slugify};

/// The result of a successful import.
pub struct ImportOutput {
    /// Directory of the generated pack.
    pub pack_dir: PathBuf,
    /// Full import report (also written to `import-report.md`).
    pub report: ImportReport,
}

/// Import a card file into a pack directory.
///
/// `out_dir` may be `None` to derive `stories/<slug>` from the card name.
/// Returns an error when the target directory exists unless
/// [`ImportOptions::force`] is set.
pub fn import_card(
    input: &Path,
    out_dir: Option<&Path>,
    options: &ImportOptions,
) -> Result<ImportOutput, String> {
    let parsed = card::load_card(input)?;
    let mut report = ImportReport::new(&parsed.canonical.name, &parsed.spec_label);
    let pack_dir = match out_dir {
        Some(path) => path.to_path_buf(),
        None => PathBuf::from("stories").join(scaffold::slugify(&parsed.canonical.name)),
    };
    report.pack_dir = pack_dir.clone();

    let sidecar_book: Option<NormalizedBook> = match &options.lorebook_path {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| {
                format!(
                    "World Info 파일을 읽지 못했습니다 ({}): {e}",
                    path.display()
                )
            })?;
            let mut notes = Vec::new();
            let book = worldinfo::parse_world_info(&text, &mut notes)?;
            for (subject, reason) in notes {
                report.note_approximation(&format!("{subject} — {reason}"));
            }
            Some(book)
        }
        None => None,
    };

    scaffold::emit_pack(&parsed, sidecar_book, &pack_dir, options, &mut report)?;
    report.write(&pack_dir.join("import-report.md"))?;
    Ok(ImportOutput { pack_dir, report })
}
