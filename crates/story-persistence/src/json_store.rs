//! JSON save store — slots, headers, save/load/delete (W8, ST-025..027).
//!
//! [`SaveStore`] lays out `<root>/<slot_id>.json` for each slot. Save is
//! atomic via [`crate::atomic_write::atomic_write_json`]; load verifies the
//! SHA-256 [`crate::checksum`] before use and diagnoses truncated / corrupt /
//! unsupported / invalid payloads. A slot does NOT appear in [`SaveStore::list_slots`]
//! until its write fully succeeds (slot isolation).
//!
//! `list_slots` materializes only the envelope header — the payload is
//! skipped via `serde::de::IgnoredAny` so a 5MB save costs O(header) to list.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use story_model::SaveEnvelope;
use story_model::chrono::{DateTime, Utc};

use crate::atomic_write::atomic_write_json;
use crate::checksum::{CHECKSUM_ALGORITHM, verify};
use crate::error::{SAVE_SCHEMA_VERSION, SaveError};

/// Default number of `.bak` generations kept per slot.
pub const DEFAULT_BAK_GENERATIONS: usize = 3;
/// Maximum ASCII characters accepted in a save slot identifier.
pub const MAX_SLOT_ID_LEN: usize = 64;

/// A slot id is a portable filename stem, not a path. Restricting it to this
/// small alphabet keeps save/load/delete under the configured root on every
/// supported platform.
pub fn validate_slot_id(slot_id: &str) -> bool {
    !slot_id.is_empty()
        && slot_id.len() <= MAX_SLOT_ID_LEN
        && slot_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn invalid_slot_id(slot_id: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("unsafe save slot id: {slot_id:?}"),
    )
}

/// The save store. Owns the saves directory root and the `.bak` retention
/// policy. Open via [`SaveStore::open`].
#[derive(Clone, Debug)]
pub struct SaveStore {
    root: PathBuf,
    bak_generations: usize,
}

/// Lightweight header fields used to populate the slot screen without
/// materializing the full payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotMeta {
    /// Slot id (filename stem).
    pub slot_id: String,
    /// Pack id this save belongs to.
    pub story_id: String,
    /// Wall-clock time the save was written.
    pub saved_at: DateTime<Utc>,
    /// Display label for the current chapter.
    pub chapter_label: String,
    /// Display label for the current scene.
    pub scene_label: String,
    /// Optional player-facing description.
    pub description: Option<String>,
}

impl SaveStore {
    /// Open a store rooted at `root`, creating the directory if needed.
    /// Default `.bak` retention is [`DEFAULT_BAK_GENERATIONS`].
    pub fn open(root: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(root)?;
        Ok(Self {
            root: root.to_path_buf(),
            bak_generations: DEFAULT_BAK_GENERATIONS,
        })
    }

    /// Builder: change the `.bak` retention count.
    #[must_use]
    pub fn with_bak_generations(mut self, n: usize) -> Self {
        self.bak_generations = n;
        self
    }

    /// Path on disk for a given slot id (`<root>/<slot_id>.json`).
    pub fn slot_path(&self, slot_id: &str) -> std::io::Result<PathBuf> {
        if !validate_slot_id(slot_id) {
            return Err(invalid_slot_id(slot_id));
        }
        Ok(self.root.join(format!("{slot_id}.json")))
    }

    /// Atomically write `envelope` to its slot. Computes the payload checksum
    /// if the envelope's `payload_checksum` field is empty; otherwise trusts
    /// the caller's value. **The slot does NOT appear in [`Self::list_slots`]
    /// until this returns Ok** — the atomic write either fully lands or
    /// leaves the pre-existing save byte-identical.
    pub fn save(&self, envelope: SaveEnvelope) -> std::io::Result<()> {
        // Compute / verify checksum before touching disk so a bad envelope
        // never corrupts an existing save.
        let mut env = envelope;
        if env.payload_checksum.trim().is_empty() {
            env.payload_checksum = crate::checksum::payload_checksum(&env.payload);
        }
        // Always assert the algorithm is the one we know how to verify.
        if env.checksum_algorithm != CHECKSUM_ALGORITHM {
            return Err(std::io::Error::other(format!(
                "unsupported checksum algorithm: {} (only '{CHECKSUM_ALGORITHM}' supported)",
                env.checksum_algorithm
            )));
        }

        let bytes = serde_json::to_vec_pretty(&env)?;
        let path = self.slot_path(&env.slot_id)?;
        atomic_write_json(&path, &bytes, self.bak_generations)?;
        Ok(())
    }

    /// Load and verify a slot. Order:
    ///   1. Read file bytes (missing → [`SaveError::MissingSlot`]).
    ///   2. Parse envelope JSON (syntax error → [`SaveError::TruncatedOrCorrupt`]).
    ///   3. Verify checksum (mismatch → [`SaveError::ChecksumMismatch`]; file untouched).
    ///   4. Check schema version (future → [`SaveError::UnsupportedVersion`];
    ///      older → routed through [`crate::migration`] in a later step).
    ///   5. Domain invariants (else [`SaveError::InvalidPayload`]).
    pub fn load(&self, slot_id: &str) -> Result<SaveEnvelope, SaveError> {
        if !validate_slot_id(slot_id) {
            return Err(SaveError::InvalidSlotId(slot_id.to_string()));
        }
        let path = self.slot_path(slot_id).map_err(SaveError::Io)?;
        let bytes = fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SaveError::MissingSlot(slot_id.to_string())
            } else {
                SaveError::Io(e)
            }
        })?;

        let envelope: SaveEnvelope = serde_json::from_slice(&bytes)
            .map_err(|e| SaveError::TruncatedOrCorrupt(e.to_string()))?;

        if envelope.checksum_algorithm != CHECKSUM_ALGORITHM {
            return Err(SaveError::TruncatedOrCorrupt(format!(
                "unsupported checksum algorithm: {}",
                envelope.checksum_algorithm
            )));
        }

        if !verify(&envelope.payload, &envelope.payload_checksum) {
            return Err(SaveError::ChecksumMismatch {
                slot: slot_id.to_string(),
            });
        }

        if envelope.save_schema_version > SAVE_SCHEMA_VERSION {
            return Err(SaveError::UnsupportedVersion {
                found: envelope.save_schema_version,
                supported: SAVE_SCHEMA_VERSION,
            });
        }

        // Domain invariants.
        if envelope.payload.stable_state.story_id.trim().is_empty() {
            return Err(SaveError::InvalidPayload(
                "stable_state.story_id is empty".to_string(),
            ));
        }

        // Migrate older schemas in-memory.
        let envelope = if envelope.save_schema_version < SAVE_SCHEMA_VERSION {
            let migrated = crate::migration::migrate_payload(
                envelope.payload,
                envelope.save_schema_version,
                SAVE_SCHEMA_VERSION,
            )?;
            SaveEnvelope {
                save_schema_version: SAVE_SCHEMA_VERSION,
                payload: migrated,
                ..envelope
            }
        } else {
            envelope
        };

        Ok(envelope)
    }

    /// List every slot, materializing only the envelope headers (payloads
    /// are skipped). Slots with corrupt/truncated envelopes are omitted
    /// rather than failing the whole listing — the slot screen shows what
    /// the player can actually resume.
    pub fn list_slots(&self) -> Vec<SlotMeta> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
                continue;
            };
            if ext != "json" || !validate_slot_id(stem) {
                continue;
            }
            // Skip our own backup files explicitly.
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains(".bak"))
                .unwrap_or(false)
            {
                continue;
            }
            if let Ok(meta) = read_header(&path) {
                out.push(SlotMeta {
                    slot_id: stem.to_string(),
                    story_id: meta.story_id,
                    saved_at: meta.saved_at,
                    chapter_label: meta.chapter_label,
                    scene_label: meta.scene_label,
                    description: meta.description,
                });
            }
        }
        out
    }

    /// Delete a slot's save file (idempotent: missing is OK). Does NOT touch
    /// `.bak` files — those age out via future writes, or the player can
    /// manually recover.
    pub fn delete(&self, slot_id: &str) -> std::io::Result<()> {
        let path = self.slot_path(slot_id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Deserialize only the envelope header fields; the payload is skipped via
/// `IgnoredAny` so listing a 5MB save is O(header).
fn read_header(path: &Path) -> Result<EnvelopeHeader, std::io::Error> {
    let bytes = fs::read(path)?;
    let header: EnvelopeHeader = serde_json::from_slice(&bytes)?;
    Ok(header)
}

#[derive(Deserialize)]
struct EnvelopeHeader {
    #[allow(dead_code)]
    save_schema_version: u32,
    #[allow(dead_code)]
    engine_version: String,
    story_id: String,
    #[allow(dead_code)]
    story_version: semver::Version,
    #[allow(dead_code)]
    pack_schema_version: semver::Version,
    #[allow(dead_code)]
    slot_id: String,
    description: Option<String>,
    saved_at: DateTime<Utc>,
    chapter_label: String,
    scene_label: String,
    #[allow(dead_code)]
    checksum_algorithm: String,
    #[allow(dead_code)]
    payload_checksum: String,
    #[serde(default, skip_serializing, rename = "payload")]
    _payload: serde::de::IgnoredAny,
}
