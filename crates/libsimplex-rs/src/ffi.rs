//! Safe Rust facade for the official SimpleX Chat Haskell library.
//!
//! The ABI is exported by `Simplex.Chat.Mobile`, the same interface used by the
//! official mobile, desktop and Node.js clients. All domain commands and events
//! cross the boundary as JSON; protocol and persistence logic remain in Haskell.

use std::{
    ffi::{CStr, CString, NulError, c_char, c_int, c_void},
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    ptr,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use libloading::Library;
use serde_json::Value;
use thiserror::Error;

type ChatCtrl = *mut c_void;
type HsInit = unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char);
type ChatMigrateInit =
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *mut ChatCtrl) -> *mut c_char;
#[cfg(test)]
type ChatCtrlCall = unsafe extern "C" fn(ChatCtrl) -> *mut c_char;
type ChatSendCmd = unsafe extern "C" fn(ChatCtrl, *const c_char) -> *mut c_char;
type ChatRecvWait = unsafe extern "C" fn(ChatCtrl, c_int) -> *mut c_char;

static HASKELL_RUNTIME: OnceLock<()> = OnceLock::new();

#[derive(Debug, Error)]
pub enum SimplexError {
    #[error("could not load SimpleX library {path}: {source}")]
    LoadLibrary {
        path: PathBuf,
        #[source]
        source: libloading::Error,
    },
    #[error("SimpleX library is missing symbol {symbol}: {source}")]
    MissingSymbol {
        symbol: &'static str,
        #[source]
        source: libloading::Error,
    },
    #[error("input contains a NUL byte")]
    InvalidInput(#[from] NulError),
    #[error("SimpleX returned a null pointer from {0}")]
    NullResult(&'static str),
    #[error("SimpleX returned invalid UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("SimpleX returned invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("SimpleX controller lock was poisoned")]
    ControllerPoisoned,
    #[error("receive timeout exceeds the SimpleX ABI limit")]
    TimeoutOverflow,
}

/// Locations owned by simplex-tui. No user-editable configuration file is used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimplexPaths {
    pub root: PathBuf,
    pub database_prefix: PathBuf,
}

impl SimplexPaths {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let database_prefix = root.join("simplex");
        Self {
            root,
            database_prefix,
        }
    }

    pub fn create(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

struct Symbols {
    migrate_init: ChatMigrateInit,
    #[cfg(test)]
    close_store: ChatCtrlCall,
    send_cmd: ChatSendCmd,
    recv_wait: ChatRecvWait,
}

/// Loaded official Haskell library. Keep this alive as long as any controller exists.
pub struct SimplexApi {
    // A live GHC runtime must never be dlclosed. The OS reclaims it at process exit.
    _library: ManuallyDrop<Library>,
    symbols: Symbols,
}

impl SimplexApi {
    /// Load the native SimpleX library built by the official desktop build script.
    ///
    /// # Safety
    /// `path` must point to an ABI-compatible official SimpleX Chat library and
    /// its GHC runtime dependencies must be discoverable by the dynamic loader.
    pub unsafe fn load(path: impl AsRef<Path>) -> Result<Arc<Self>, SimplexError> {
        let path = path.as_ref();
        // GHC's dynamically linked RTS requires its symbols in the global namespace.
        // SAFETY: the caller guarantees this is a compatible SimpleX library.
        #[cfg(unix)]
        let library: Library = unsafe {
            libloading::os::unix::Library::open(Some(path), libc::RTLD_NOW | libc::RTLD_GLOBAL)
        }
        .map(Into::into)
        .map_err(|source| SimplexError::LoadLibrary {
            path: path.to_path_buf(),
            source,
        })?;
        #[cfg(not(unix))]
        let library =
            unsafe { Library::new(path) }.map_err(|source| SimplexError::LoadLibrary {
                path: path.to_path_buf(),
                source,
            })?;

        // Copying function pointers lets the symbols borrow no longer than this scope;
        // `_library` keeps their code loaded for the lifetime of `SimplexApi`.
        let hs_init: HsInit =
            unsafe { symbol(&library, b"hs_init_with_rtsopts\0", "hs_init_with_rtsopts")? };
        let symbols = Symbols {
            migrate_init: unsafe { symbol(&library, b"chat_migrate_init\0", "chat_migrate_init")? },
            #[cfg(test)]
            close_store: unsafe { symbol(&library, b"chat_close_store\0", "chat_close_store")? },
            send_cmd: unsafe { symbol(&library, b"chat_send_cmd\0", "chat_send_cmd")? },
            recv_wait: unsafe { symbol(&library, b"chat_recv_msg_wait\0", "chat_recv_msg_wait")? },
        };

        HASKELL_RUNTIME.get_or_init(|| {
            // GHC 9.6.3's non-moving collector (-xn) crashes in evacuate when
            // this library is embedded in the Rust process. Use the standard
            // moving collector; allocation and initial heap sizes still match
            // the official desktop wrapper.
            let arguments = [
                "simplex",
                "+RTS",
                "-A64m",
                "-H64m",
                "--install-signal-handlers=no",
            ];
            let c_arguments: Vec<CString> = arguments
                .iter()
                .map(|argument| CString::new(*argument).expect("static RTS argument contains NUL"))
                .collect();
            let mut pointers: Vec<*mut c_char> = c_arguments
                .iter()
                .map(|argument| argument.as_ptr().cast_mut())
                .chain(std::iter::once(ptr::null_mut()))
                .collect();
            let mut argc = arguments.len() as c_int;
            let mut argv = pointers.as_mut_ptr();
            // SAFETY: argv is writable, NUL-terminated, and remains alive for the call.
            unsafe { hs_init(&mut argc, &mut argv) };
        });

        Ok(Arc::new(Self {
            _library: ManuallyDrop::new(library),
            symbols,
        }))
    }

    pub fn open(
        self: &Arc<Self>,
        database_prefix: &Path,
        database_key: &str,
        migration_confirmation: &str,
    ) -> Result<(SimplexController, Value), SimplexError> {
        let path = cstring_path(database_prefix)?;
        let key = CString::new(database_key)?;
        let confirmation = CString::new(migration_confirmation)?;
        let mut controller = ptr::null_mut();
        // SAFETY: strings are valid for the call and controller is a writable out pointer.
        let result = unsafe {
            (self.symbols.migrate_init)(
                path.as_ptr(),
                key.as_ptr(),
                confirmation.as_ptr(),
                &mut controller,
            )
        };
        let migration = unsafe { take_json(result, "chat_migrate_init")? };
        if controller.is_null() {
            return Err(SimplexError::NullResult("chat_migrate_init controller"));
        }
        Ok((
            SimplexController {
                api: Arc::clone(self),
                raw: Mutex::new(controller),
            },
            migration,
        ))
    }
}

/// Thread-safe owner of a Haskell `ChatController` stable pointer.
pub struct SimplexController {
    api: Arc<SimplexApi>,
    raw: Mutex<ChatCtrl>,
}

// The mutex serializes access to the opaque stable pointer. SimpleX itself owns
// its worker threads and communicates through its command/event queues.
unsafe impl Send for SimplexController {}
unsafe impl Sync for SimplexController {}

impl SimplexController {
    /// Execute any command supported by the official SimpleX chat command API.
    pub fn command(&self, command: &str) -> Result<Value, SimplexError> {
        let command = CString::new(command)?;
        let ctrl = *self
            .raw
            .lock()
            .map_err(|_| SimplexError::ControllerPoisoned)?;
        // SAFETY: the controller remains owned by self and command lives through the call.
        unsafe {
            take_json(
                (self.api.symbols.send_cmd)(ctrl, command.as_ptr()),
                "chat_send_cmd",
            )
        }
    }

    /// Wait for the next asynchronous SimpleX event. An empty string means timeout.
    pub fn recv(&self, timeout: Duration) -> Result<Option<Value>, SimplexError> {
        let micros: c_int = timeout
            .as_micros()
            .try_into()
            .map_err(|_| SimplexError::TimeoutOverflow)?;
        let ctrl = *self
            .raw
            .lock()
            .map_err(|_| SimplexError::ControllerPoisoned)?;
        // SAFETY: the controller remains owned by self for the duration of the call.
        unsafe {
            take_optional_json(
                (self.api.symbols.recv_wait)(ctrl, micros),
                "chat_recv_msg_wait",
            )
        }
    }

    #[cfg(test)]
    pub fn close_store(&self) -> Result<String, SimplexError> {
        self.store_call(self.api.symbols.close_store, "chat_close_store")
    }

    #[cfg(test)]
    fn store_call(&self, call: ChatCtrlCall, name: &'static str) -> Result<String, SimplexError> {
        let ctrl = *self
            .raw
            .lock()
            .map_err(|_| SimplexError::ControllerPoisoned)?;
        // SAFETY: the controller remains owned by self for the duration of the call.
        unsafe { take_string(call(ctrl), name) }
    }
}

unsafe fn symbol<T: Copy>(
    library: &Library,
    name: &[u8],
    display_name: &'static str,
) -> Result<T, SimplexError> {
    // SAFETY: callers provide the exact function type defined by the upstream C ABI.
    unsafe { library.get::<T>(name) }
        .map(|symbol| *symbol)
        .map_err(|source| SimplexError::MissingSymbol {
            symbol: display_name,
            source,
        })
}

fn cstring_path(path: &Path) -> Result<CString, SimplexError> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(Into::into)
}

unsafe fn take_string(pointer: *mut c_char, name: &'static str) -> Result<String, SimplexError> {
    if pointer.is_null() {
        return Err(SimplexError::NullResult(name));
    }
    // SAFETY: upstream returns a malloc-allocated, NUL-terminated CString.
    let result = unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map(str::to_owned);
    // SAFETY: upstream's official Node.js binding frees these results with libc free.
    unsafe { libc::free(pointer.cast()) };
    Ok(result?)
}

unsafe fn take_json(pointer: *mut c_char, name: &'static str) -> Result<Value, SimplexError> {
    let value = unsafe { take_string(pointer, name)? };
    Ok(serde_json::from_str(&value)?)
}

unsafe fn take_optional_json(
    pointer: *mut c_char,
    name: &'static str,
) -> Result<Option<Value>, SimplexError> {
    let value = unsafe { take_string(pointer, name)? };
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::from_str(&value)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_data_lives_under_one_root() {
        let paths = SimplexPaths::at("/tmp/example-home/.simplex-tui");
        assert_eq!(
            paths.database_prefix,
            PathBuf::from("/tmp/example-home/.simplex-tui/simplex")
        );
    }

    #[test]
    #[ignore = "requires the separately built official Haskell library"]
    fn loads_official_haskell_runtime() {
        let library = std::env::var_os("SIMPLEX_LIBRARY")
            .map(PathBuf::from)
            .unwrap_or_else(crate::bundled_library_path);
        // SAFETY: this test explicitly targets the pinned library built by our bootstrap script.
        let api = unsafe { SimplexApi::load(library) };
        assert!(api.is_ok(), "{:#}", api.err().expect("error exists"));
    }

    #[test]
    #[ignore = "requires the separately built official Haskell library"]
    fn opens_database_and_calls_chat_api() {
        let library = std::env::var_os("SIMPLEX_LIBRARY")
            .map(PathBuf::from)
            .unwrap_or_else(crate::bundled_library_path);
        // SAFETY: this test explicitly targets the pinned library built by our bootstrap script.
        let api = unsafe { SimplexApi::load(library) }.expect("load SimpleX library");
        let temp = tempfile::tempdir().expect("create temporary data directory");
        let (controller, migration) = api
            .open(&temp.path().join("simplex"), "", "yesUp")
            .expect("initialize SimpleX database");
        assert_eq!(migration.get("type").and_then(Value::as_str), Some("ok"));
        controller.command("/u").expect("query active user");
        controller.close_store().expect("close SimpleX database");
    }

    #[test]
    #[ignore = "requires the separately built official Haskell library"]
    fn creates_lists_and_activates_profile() {
        let library = crate::bundled_library_path();
        // SAFETY: this test explicitly targets the pinned library built by our bootstrap script.
        let api = unsafe { SimplexApi::load(library) }.expect("load SimpleX library");
        let temp = tempfile::tempdir().expect("create temporary data directory");
        let (controller, _) = api
            .open(&temp.path().join("simplex"), "", "yesUp")
            .expect("initialize SimpleX database");
        let created = controller
            .command(
                "/_create user {\"profile\":{\"displayName\":\"Test User\",\"fullName\":\"\"},\"pastTimestamp\":false}",
            )
            .expect("create profile");
        let user_id = created
            .pointer("/result/user/userId")
            .and_then(Value::as_i64)
            .expect("created user id");
        controller.command("/_start").expect("start chat");
        let users = controller.command("/users").expect("list profiles");
        assert_eq!(
            users.pointer("/result/type").and_then(Value::as_str),
            Some("usersList")
        );
        controller
            .command(&format!("/_user {user_id}"))
            .expect("activate profile");
        controller.close_store().expect("close SimpleX database");
    }
}
