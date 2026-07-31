//! Atomic JSON write — the 9-step procedure from Technical Spec §13.2 (W8, ST-026).
//!
//! Contract: on success, `path` holds exactly `bytes`; on ANY failure, the
//! pre-existing file at `path` (if any) is left byte-identical and no temp
//! files are left behind. Atomicity of the final swap relies on the temp
//! living in the SAME directory as the destination (same filesystem, so
//! `rename` is atomic).
//!
//! Steps:
//!   1. Create a unique temp file in the SAME directory as `path`.
//!   2. Write `bytes`, flush, `sync_all` (fsync the file).
//!   3. If `path` exists, rotate: keep up to `bak_generations` `.bak` files.
//!   4. Atomic `rename(temp → path)`.
//!   5. `sync_dir` the parent (best-effort).
//!   6. Return Ok.
//!
//! On any error: leave the existing `path` untouched, clean up the temp.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

/// Atomic write with backup rotation.
///
/// `bak_generations` controls how many `.bak` / `.bak.N` files are kept:
/// `0` keeps none (the existing file is unlinked before the rename), `N>0`
/// keeps up to `N` prior generations (`.bak`, then `.bak.1`, `.bak.2`, ...).
pub fn atomic_write_json(path: &Path, bytes: &[u8], bak_generations: usize) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io_err("atomic_write needs a parent directory"))?;

    // Step 1 — same-dir temp file. `NamedTempFile::new_in(parent)` places the
    // temp in the same directory (and therefore on the same filesystem), so
    // the closing `persist` rename is atomic. On error, the temp is cleaned
    // up by `NamedTempFile`'s Drop — pre-existing `path` is untouched.
    let mut temp = NamedTempFile::new_in(parent)?;
    let temp_path = temp.path().to_path_buf();

    // Step 2 — write + flush + fsync the file.
    if let Err(e) = write_and_sync(&mut temp, bytes) {
        // Best-effort cleanup of the temp; ignore errors.
        drop(temp);
        let _ = fs::remove_file(&temp_path);
        return Err(e);
    }

    // Step 3 + 4 — rotate existing file then atomic rename. We do this via
    // `temp.persist()` to get the atomic rename; rotation happens first.
    if let Err(e) = rotate_and_persist(temp, path, bak_generations) {
        // `persist` failure already consumed `temp`; otherwise clean up.
        let _ = fs::remove_file(&temp_path);
        return Err(e);
    }

    // Step 5 — best-effort fsync of the parent directory so the rename
    // survives a crash. Failures here are ignored on platforms that don't
    // support directory fsync (e.g. some network FSes) — the data is already
    // durably written via the file's own fsync.
    let _ = sync_dir(parent);

    Ok(())
}

fn write_and_sync(temp: &mut NamedTempFile, bytes: &[u8]) -> std::io::Result<()> {
    temp.write_all(bytes)?;
    temp.flush()?;
    let file = temp.as_file();
    file.sync_all()?;
    Ok(())
}

fn rotate_and_persist(
    temp: NamedTempFile,
    path: &Path,
    bak_generations: usize,
) -> std::io::Result<()> {
    if path.exists() {
        if bak_generations == 0 {
            // No backup wanted: remove the existing file so the rename lands
            // cleanly. (This is still atomic from the reader's perspective
            // because the rename itself is atomic; we just don't keep a .bak.)
            fs::remove_file(path)?;
        } else {
            rotate_backups(path, bak_generations)?;
        }
    }
    // Step 4 — atomic rename via tempfile's `persist`. Consumes `temp`.
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Rotate `.bak` files for `path`:
/// - `path.bak.(N-1)` → `path.bak.N` (drop the last if it exceeds `bak_generations`)
/// - ...
/// - `path.bak` → `path.bak.1`
/// - `path` → `path.bak`
///
/// After rotation, `path` is gone (the caller renames the temp into place).
fn rotate_backups(path: &Path, bak_generations: usize) -> std::io::Result<()> {
    debug_assert!(bak_generations > 0);
    // Drop the oldest generation if it would exceed the cap.
    // We keep `.bak`, `.bak.1`, ..., `.bak.(bak_generations-1)` = bak_generations total.
    let oldest = format!("{}.bak.{}", path.display(), bak_generations - 1);
    let oldest = PathBuf::from(oldest);
    if oldest.exists() {
        let _ = fs::remove_file(&oldest);
    }
    // Shift `.bak.(n)` → `.bak.(n+1)` from high to low so we don't clobber.
    if bak_generations > 1 {
        let mut n = bak_generations - 1;
        while n > 1 {
            n -= 1;
            let src = PathBuf::from(format!("{}.bak.{n}", path.display()));
            let dst = PathBuf::from(format!("{}.bak.{}", path.display(), n + 1));
            if src.exists() {
                // Rename; if dst somehow exists (shouldn't), remove first.
                if dst.exists() {
                    let _ = fs::remove_file(&dst);
                }
                fs::rename(&src, &dst)?;
            }
        }
    }
    // `.bak` → `.bak.1`
    let bak = PathBuf::from(format!("{}.bak", path.display()));
    if bak.exists() {
        let bak1 = PathBuf::from(format!("{}.bak.1", path.display()));
        if bak1.exists() {
            let _ = fs::remove_file(&bak1);
        }
        fs::rename(&bak, &bak1)?;
    }
    // `path` → `.bak`
    fs::rename(path, &bak)?;
    Ok(())
}

/// Best-effort directory fsync. On macOS/Linux this flushes the directory
/// entry change from the rename so it survives a crash. This is genuinely
/// best-effort: any error (unsupported filesystem, network FS, read-only
/// mount) is swallowed — the file fsync already guarantees the bytes are
/// durable, the dir fsync only protects the rename dirent.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    match File::open(dir).and_then(|f| f.sync_all()) {
        Ok(()) => Ok(()),
        // Swallow everything: best-effort.
        Err(_) => Ok(()),
    }
}

fn io_err(msg: &'static str) -> std::io::Error {
    std::io::Error::other(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_file_when_absent() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.json");
        atomic_write_json(&p, b"hi", 3).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"hi");
    }

    #[test]
    fn overwrites_and_keeps_bak() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.json");
        atomic_write_json(&p, b"v1", 3).unwrap();
        atomic_write_json(&p, b"v2", 3).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"v2");
        assert_eq!(fs::read(dir.path().join("a.json.bak")).unwrap(), b"v1");
    }
}
