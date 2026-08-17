use crate::app::App;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use std::io::stdout;
use std::{path::PathBuf, sync::Arc};

pub mod app;
pub mod chat;
pub mod event;
pub mod preferences;
pub mod simplex;
pub mod simplex_worker;
pub mod ui;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let library_path = std::env::var_os("SIMPLEX_CHAT_LIB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor/libsimplex/libsimplex.so")
        });
    // Initialize the GHC runtime on the process main thread, before Tokio
    // creates any worker threads. The bootstrap script pins the matching ABI.
    let api = unsafe { simplex::SimplexApi::load(library_path) }?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(api))
}

async fn run(api: Arc<simplex::SimplexApi>) -> color_eyre::Result<()> {
    let terminal = ratatui::init();
    execute!(
        stdout(),
        EnableMouseCapture,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    let result = App::new(api).run(terminal).await;
    let input_result = execute!(stdout(), PopKeyboardEnhancementFlags, DisableMouseCapture);
    ratatui::restore();
    input_result?;
    result
}
