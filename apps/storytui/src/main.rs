//! StoryTUI binary entrypoint.
//!
//! StoryTUI executable: configuration, terminal lifecycle, engine effects, and
//! the real OpenAI-compatible provider are assembled here.

mod host;

use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::Mutex;

#[tokio::main]
async fn main() {
    let cli = match storytui::config::CliOptions::parse(std::env::args().skip(1).collect()) {
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
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(save_dir.join("storytui.log"))
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
