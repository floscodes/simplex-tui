//! Safe, typed Rust interface to the official SimpleX Chat library.
//!
//! The native ABI and textual SimpleX command language are implementation
//! details of this crate. Consumers interact only with Rust data types,
//! commands and events.

mod client;
mod ffi;
mod model;

use std::{path::Path, path::PathBuf, sync::Arc, sync::mpsc};

pub use client::{ChatDeleteMode, ChatFeature, SimplexCommand as Command};
pub use ffi::SimplexError as Error;
pub use model::{
    Attachment, AttachmentKind, ChatDeletionSettings, ChatFeatures, ChatRef, ChatSummary, Message,
    MessageReaction, Profile, ServerEntry, ServerProtocol, SimplexEvent as Event, User,
};

/// Loaded official SimpleX Chat runtime.
pub struct Client {
    api: Arc<ffi::SimplexApi>,
}

/// Filesystem locations selected by the embedding application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub data_directory: PathBuf,
    pub download_directory: PathBuf,
}

impl Config {
    pub fn new(data_directory: impl Into<PathBuf>, download_directory: impl Into<PathBuf>) -> Self {
        Self {
            data_directory: data_directory.into(),
            download_directory: download_directory.into(),
        }
    }
}

impl Client {
    /// Load an official, ABI-compatible SimpleX Chat native library.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        // SAFETY: symbol lookup and ownership are validated and encapsulated by
        // `SimplexApi`; callers only receive the safe typed wrapper.
        let api = unsafe { ffi::SimplexApi::load(path) }?;
        Ok(Self { api })
    }

    /// Load the native library produced by this workspace's bundled bootstrap.
    pub fn load_bundled() -> Result<Self, Error> {
        Self::load(bundled_library_path())
    }

    /// Start the serialized SimpleX controller and its typed command/event bridge.
    pub fn start(self, config: Config) -> Session {
        let (event_sender, events) = mpsc::channel();
        let commands = client::spawn(self.api, config, event_sender);
        Session { commands, events }
    }
}

/// Typed command and event endpoints for one SimpleX client session.
pub struct Session {
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<Event>,
}

impl Session {
    pub fn into_parts(self) -> (mpsc::Sender<Command>, mpsc::Receiver<Event>) {
        (self.commands, self.events)
    }
}

pub fn bundled_library_path() -> std::path::PathBuf {
    let name = if cfg!(target_os = "linux") {
        "libsimplex.so"
    } else if cfg!(target_os = "macos") {
        "libsimplex.dylib"
    } else {
        "libsimplex.dll"
    };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vendor/libsimplex")
        .join(name)
}
