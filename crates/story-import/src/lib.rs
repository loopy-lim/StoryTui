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
pub mod enrich;
pub mod macros;
pub mod normalize;
pub mod report;
pub mod scaffold;
pub mod worldinfo;

use std::path::{Path, PathBuf};

use report::ImportReport;
use worldinfo::NormalizedBook;

pub use enrich::{CharacterEnrichment, enrich_from_card};
pub use scaffold::{ImportOptions, slugify};

/// The result of a successful import.
pub struct ImportOutput {
    /// Directory of the generated pack.
    pub pack_dir: PathBuf,
    /// Full import report (also written to `import-report.md`).
    pub report: ImportReport,
}

/// Aggregate result for [`import_inbox`]: partial success is allowed.
#[derive(Default)]
pub struct InboxImportResult {
    pub ok: Vec<ImportOutput>,
    pub errors: Vec<(PathBuf, String)>,
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

/// Import every supported card in `dir` (non-recursive): `*.png`, `*.json`,
/// `*.charx`. Each file gets `stories/<slug>` (or fails individually). Existing
/// packs are skipped unless [`ImportOptions::force`] is set.
pub fn import_inbox(dir: &Path, options: &ImportOptions) -> Result<InboxImportResult, String> {
    if !dir.is_dir() {
        return Err(format!(
            "인박스 디렉터리가 아닙니다: {}",
            dir.display()
        ));
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("인박스를 읽지 못했습니다 ({}): {e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| {
                        let ext = ext.to_ascii_lowercase();
                        matches!(ext.as_str(), "png" | "json" | "charx")
                    })
                    .unwrap_or(false)
        })
        .collect();
    entries.sort();
    if entries.is_empty() {
        return Err(format!(
            "인박스에 카드 파일이 없습니다 (*.png / *.json / *.charx): {}",
            dir.display()
        ));
    }

    let mut result = InboxImportResult::default();
    for card_path in entries {
        match import_card(&card_path, None, options) {
            Ok(output) => result.ok.push(output),
            Err(error) => result.errors.push((card_path, error)),
        }
    }
    Ok(result)
}

/// Korean summary line when some inbox imports failed (partial success OK).
pub fn inbox_error_summary(errors: &[(PathBuf, String)]) -> String {
    if errors.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!("실패한 카드 {}개:", errors.len())];
    for (path, error) in errors {
        lines.push(format!("  - {}: {error}", path.display()));
    }
    lines.join("\n")
}
