use crate::app::App;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use directories::{BaseDirs, UserDirs};
use libsimplex_rs::{Client, Config};
use std::io::stdout;

pub mod app;
pub mod event;
pub mod preferences;
pub mod ui;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    // Initialize the GHC runtime on the process main thread, before Tokio
    // creates any worker threads. The bootstrap script pins the matching ABI.
    let client = match std::env::var_os("SIMPLEX_CHAT_LIB") {
        Some(path) => Client::load(path)?,
        None => Client::load(bundled_library_path())?,
    };
    let base_dirs = BaseDirs::new().ok_or_else(|| color_eyre::eyre::eyre!("no home directory"))?;
    let data_directory = base_dirs.home_dir().join(".simplex-tui");
    let download_directory = UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| base_dirs.home_dir().join("Downloads"));
    let session = client.start(Config::new(&data_directory, download_directory));
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(session, data_directory))
}

fn bundled_library_path() -> std::path::PathBuf {
    let name = if cfg!(target_os = "linux") {
        "libsimplex.so"
    } else if cfg!(target_os = "macos") {
        "libsimplex.dylib"
    } else {
        "libsimplex.dll"
    };
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        let beside_executable = directory.join(name);
        if beside_executable.is_file() {
            return beside_executable;
        }
        let library_directory = directory.join("lib").join(name);
        if library_directory.is_file() {
            return library_directory;
        }
    }

    // Development fallback for `cargo run` and local test builds.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vendor/libsimplex")
        .join(name)
}

async fn run(
    session: libsimplex_rs::Session,
    data_directory: std::path::PathBuf,
) -> color_eyre::Result<()> {
    let terminal = ratatui::init();
    execute!(
        stdout(),
        EnableMouseCapture,
        EnableBracketedPaste,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    let result = App::new(session, data_directory).run(terminal).await;
    let input_result = execute!(
        stdout(),
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        DisableMouseCapture
    );
    ratatui::restore();
    input_result?;
    result
}
