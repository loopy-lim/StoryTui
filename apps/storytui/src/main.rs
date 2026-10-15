//! StoryTUI binary entrypoint.
//!
//! StoryTUI executable: configuration, terminal lifecycle, engine effects, and
//! the real OpenAI-compatible provider are assembled here.

mod host;
mod import_cmd;

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Hard cap for the local runtime trace. When the next formatted event would
/// cross the cap the file starts fresh, so a long-running TUI cannot consume
/// the save volume without bound.
const MAX_RUNTIME_LOG_BYTES: u64 = 1024 * 1024;

#[tokio::main]
async fn main() {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    // The import subcommand never enters the TUI: it compiles a card into a
    // pack directory and prints a report.
    if raw_args.first().map(String::as_str) == Some("import") {
        if let Err(message) = import_cmd::run(&raw_args[1..]).await {
            eprintln!("{message}");
            std::process::exit(2);
        }
        return;
    }
    let cli = match storytui::config::CliOptions::parse(raw_args) {
        Ok(cli) => cli,
        Err(message) if message == storytui::config::CliOptions::usage() => {
            println!("{message}");
            return;
        }
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let config = match storytui::config::load(&cli) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("설정 오류: {message}");
            std::process::exit(2);
        }
    };
    init_runtime_logging(&config.save_dir);
    if let Some(path) = &config.source_path {
        tracing::info!(config = %path.display(), "loaded StoryTUI config");
    }
    if let Err(error) = host::run(config).await {
        eprintln!("StoryTUI 실행 실패: {error}");
        std::process::exit(1);
    }
}

/// TUI diagnostics must never be written to stdout/stderr while Ratatui owns
/// the alternate screen. Send them to the local save directory instead; if
/// that directory is unavailable, deliberately discard them rather than
/// corrupting the player's terminal.
fn init_runtime_logging(save_dir: &Path) {
    let log_file = fs::create_dir_all(save_dir).and_then(|()| {
        BoundedLogWriter::open(save_dir.join("storytui.log"), MAX_RUNTIME_LOG_BYTES)
    });
    match log_file {
        Ok(file) => tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .init(),
        Err(_) => tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(io::sink)
            .init(),
    }
}

/// File writer with a strict on-disk size ceiling. It deliberately truncates
/// the previous trace at a rollover instead of keeping backup generations:
/// diagnostics are best-effort metadata, not player data.
struct BoundedLogWriter {
    path: PathBuf,
    file: File,
    written: u64,
    max_bytes: u64,
}

impl BoundedLogWriter {
    fn open(path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        if max_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "runtime log cap must be greater than zero",
            ));
        }
        let mut options = OpenOptions::new();
        options.create(true).write(true);
        let existing = fs::metadata(&path).map_or(0, |metadata| metadata.len());
        if existing > max_bytes {
            options.truncate(true);
        } else {
            options.append(true);
        }
        let file = options.open(&path)?;
        Ok(Self {
            path,
            file,
            written: existing.min(max_bytes),
            max_bytes,
        })
    }

    fn truncate(&mut self) -> io::Result<()> {
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

impl Write for BoundedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if buf.len() as u64 > self.max_bytes {
            self.truncate()?;
            let keep = self.max_bytes as usize;
            self.file.write_all(&buf[buf.len() - keep..])?;
            self.written = self.max_bytes;
            return Ok(buf.len());
        }
        if self.written.saturating_add(buf.len() as u64) > self.max_bytes {
            self.truncate()?;
        }
        self.file.write_all(buf)?;
        self.written += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_log_writer_never_exceeds_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("storytui.log");
        let mut writer = BoundedLogWriter::open(path.clone(), 16).expect("writer");
        writer.write_all(b"1234567890").expect("first write");
        writer.write_all(b"abcdefghij").expect("rollover write");
        writer.flush().expect("flush");
        assert_eq!(fs::read(&path).expect("read"), b"abcdefghij");

        writer
            .write_all(b"0123456789abcdefghijkl")
            .expect("oversized write");
        writer.flush().expect("flush oversized");
        let bytes = fs::read(path).expect("read oversized");
        assert_eq!(bytes.len(), 16);
        assert_eq!(bytes, b"6789abcdefghijkl");
    }
}
