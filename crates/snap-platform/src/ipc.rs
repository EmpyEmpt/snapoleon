//! Single-instance IPC.
//!
//! Transport is a local socket (named pipe `\\.\pipe\snapoleon-<key>` on
//! Windows, created with a DACL that admits only the current user and checked by the client
//! against the server process's user SID before anything is sent; a Unix domain socket in
//! `$XDG_RUNTIME_DIR` or a private temp directory elsewhere) carrying one newline-delimited JSON
//! [`Request`] and one [`Response`] per connection.
//!
//! [`serve`] doubles as the single-instance check: it fails with
//! [`AlreadyRunning`] when another process already owns the endpoint.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::Listener as _;
use interprocess::local_socket::{Listener, ListenerOptions, Name, Stream};
use snap_core::ipc::{Request, Response};
use snap_core::{Error, Result};

use crate::util::other;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// A request or response larger than this is rejected.
const MAX_MESSAGE: u64 = 4 * 1024 * 1024;

/// Returned (wrapped in [`Error::Other`]) by [`serve`] when another instance owns the endpoint.
/// Use [`is_already_running`] to test for it.
#[derive(Debug)]
pub struct AlreadyRunning;

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("another Snapoleon instance is already running")
    }
}

impl std::error::Error for AlreadyRunning {}

/// Whether an error from [`serve`] means "another instance is running".
pub fn is_already_running(e: &Error) -> bool {
    matches!(e, Error::Other(inner) if inner.is::<AlreadyRunning>())
}

/// The client end of a connection. On Windows it is a plain file handle, opened for identification only and checked
/// against the server's user; `interprocess` cannot set those flags.
#[cfg(windows)]
type Client = std::fs::File;
#[cfg(unix)]
type Client = Stream;

type Handler = Arc<dyn Fn(Request) -> Response + Send + Sync>;

/// A running IPC server. Dropping it stops accepting connections and frees the endpoint.
pub struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocked accept().
        let _ = connect_once();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start serving requests on a background thread; each connection is handled on
/// its own thread, so `handler` may block (for example while an overlay is open).
pub fn serve(handler: impl Fn(Request) -> Response + Send + Sync + 'static) -> Result<Server> {
    let listener = bind()?;
    let handler: Handler = Arc::new(handler);
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let stop = stop.clone();
        std::thread::Builder::new().name("snap-ipc".into()).spawn(move || accept_loop(listener, handler, stop))?
    };
    Ok(Server { stop, thread: Some(thread) })
}

/// Send a request to the running instance and wait for its response.
/// Fails with [`Error::NotFound`] when no instance is running.
pub fn send(req: &Request) -> Result<Response> {
    let stream = connect(CONNECT_TIMEOUT).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => Error::NotFound("running Snapoleon instance".into()),
        _ => Error::Io(e),
    })?;
    let mut line = serde_json::to_string(req).map_err(other)?;
    line.push('\n');
    let mut writer = &stream;
    writer.write_all(line.as_bytes())?;
    writer.flush()?;

    let mut reply = String::new();
    BufReader::new(&stream).take(MAX_MESSAGE).read_line(&mut reply)?;
    if reply.trim().is_empty() {
        return Err(other("the running instance closed the connection without a response"));
    }
    serde_json::from_str(reply.trim()).map_err(other)
}

/// Whether another instance is currently serving.
pub fn instance_running() -> bool {
    connect_once().is_ok()
}

fn accept_loop(listener: Listener, handler: Handler, stop: Arc<AtomicBool>) {
    loop {
        let accepted = listener.accept();
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match accepted {
            Ok(stream) => {
                let handler = handler.clone();
                if let Err(e) = std::thread::Builder::new().name("snap-ipc-conn".into()).spawn(move || handle(stream, &handler)) {
                    tracing::warn!("ipc: cannot spawn connection thread: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("ipc accept: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn handle(stream: Stream, handler: &Handler) {
    let mut line = String::new();
    match BufReader::new(&stream).take(MAX_MESSAGE).read_line(&mut line) {
        // A probe from `instance_running` or the shutdown wake-up: nothing to answer.
        Ok(0) | Err(_) => return,
        Ok(_) => {}
    }
    let response = match serde_json::from_str::<Request>(line.trim()) {
        Ok(req) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(req)))
            .unwrap_or_else(|_| Response::err("request handler panicked")),
        Err(e) => Response::err(format!("invalid request: {e}")),
    };
    match serde_json::to_string(&response) {
        Ok(mut out) => {
            out.push('\n');
            let mut writer = &stream;
            if let Err(e) = writer.write_all(out.as_bytes()).and_then(|_| writer.flush()) {
                tracing::debug!("ipc reply: {e}");
            }
        }
        Err(e) => tracing::warn!("ipc reply: {e}"),
    }
}

fn bind() -> Result<Listener> {
    let name = socket_name()?;
    let err = match listener_options(name.clone())?.create_sync() {
        Ok(listener) => return Ok(listener),
        Err(e) => e,
    };
    if !matches!(err.kind(), std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied) {
        return Err(Error::Io(err));
    }
    // Windows refuses a second pipe server outright. On Unix the socket file may
    // merely be stale: only a refused connection proves that.
    if connect_once().is_ok() {
        return Err(Error::Other(anyhow::Error::new(AlreadyRunning)));
    }
    reclaim_stale(name, err)
}

#[cfg(unix)]
fn reclaim_stale(name: Name<'static>, _err: std::io::Error) -> Result<Listener> {
    let _ = std::fs::remove_file(socket_path().map_err(Error::Io)?);
    listener_options(name)?.create_sync().map_err(Error::Io)
}

/// The pipe exists but nobody we trust serves it: another account holds the name.
#[cfg(windows)]
fn reclaim_stale(_name: Name<'static>, err: std::io::Error) -> Result<Listener> {
    Err(Error::Io(err))
}

#[cfg(unix)]
fn listener_options(name: Name<'static>) -> Result<ListenerOptions<'static>> {
    Ok(ListenerOptions::new().name(name))
}

/// Only the current user may open the pipe; the default DACL would also admit other accounts.
#[cfg(windows)]
fn listener_options(name: Name<'static>) -> Result<ListenerOptions<'static>> {
    use interprocess::os::windows::local_socket::ListenerOptionsExt as _;
    Ok(ListenerOptions::new().name(name).security_descriptor(win::owner_only_descriptor().map_err(Error::Io)?))
}

fn connect_once() -> std::io::Result<Client> {
    open_client()
}

#[cfg(unix)]
fn open_client() -> std::io::Result<Client> {
    use interprocess::local_socket::ConnectOptions;
    ConnectOptions::new().name(socket_name().map_err(|e| std::io::Error::other(e.to_string()))?).connect_sync()
}

/// Open the pipe so the server can only identify us (never impersonate), and use it only if its process runs as this
/// user. Any other server is treated as no server: whoever holds the name is not our instance.
#[cfg(windows)]
fn open_client() -> std::io::Result<Client> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::SECURITY_IDENTIFICATION;

    let path = format!(r"\\.\pipe\snapoleon-{}", win::endpoint_key()?);
    let pipe = std::fs::OpenOptions::new().read(true).write(true).security_qos_flags(SECURITY_IDENTIFICATION.0).open(path)?;
    match win::served_by_current_user(&pipe) {
        Ok(true) => Ok(pipe),
        Ok(false) => {
            tracing::warn!("ipc: the pipe is served by another user; treating Snapoleon as not running");
            Err(std::io::ErrorKind::NotFound.into())
        }
        Err(e) => {
            tracing::warn!("ipc: cannot identify the pipe's server ({e}); treating Snapoleon as not running");
            Err(std::io::ErrorKind::NotFound.into())
        }
    }
}

/// Connect, retrying briefly while the server is between pipe instances.
fn connect(timeout: Duration) -> std::io::Result<Client> {
    let deadline = Instant::now() + timeout;
    loop {
        match open_client() {
            Ok(s) => return Ok(s),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => return Err(e),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

#[cfg(windows)]
fn socket_name() -> Result<Name<'static>> {
    use interprocess::local_socket::{GenericNamespaced, ToNsName};
    format!("snapoleon-{}", win::endpoint_key().map_err(Error::Io)?).to_ns_name::<GenericNamespaced>().map_err(Error::Io)
}

#[cfg(windows)]
mod win {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use interprocess::os::windows::security_descriptor::SecurityDescriptor;
    use widestring::U16CString;
    use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetLengthSid, GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};

    /// A descriptor whose only ACE gives the current user's SID full access, with inheritance from the parent blocked.
    pub fn owner_only_descriptor() -> std::io::Result<SecurityDescriptor> {
        let sid = user_sid(unsafe { GetCurrentProcess() })?;
        let mut text = windows::core::PWSTR::null();
        // SAFETY: `sid` is a valid SID copied out of a token, and `text` receives a LocalAlloc'd string.
        unsafe { ConvertSidToStringSidW(PSID(sid.as_ptr() as *mut _), &mut text)? };
        let sid_text = unsafe { text.to_string() };
        // SAFETY: the string came from ConvertSidToStringSidW and is not used again.
        unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        let sddl =
            U16CString::from_str(format!("D:P(A;;GA;;;{})", sid_text.map_err(std::io::Error::other)?)).map_err(std::io::Error::other)?;
        SecurityDescriptor::deserialize(&sddl)
    }

    /// Whether the process serving `pipe` runs as the same user as this one.
    pub fn served_by_current_user(pipe: &std::fs::File) -> std::io::Result<bool> {
        let mut pid = 0u32;
        // SAFETY: the handle is a live pipe handle owned by `pipe`.
        unsafe { GetNamedPipeServerProcessId(HANDLE(pipe.as_raw_handle()), &mut pid)? };
        // SAFETY: the handle returned is owned from here on.
        let server = unsafe { OwnedHandle::from_raw_handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?.0) };
        Ok(user_sid(HANDLE(server.as_raw_handle()))? == user_sid(unsafe { GetCurrentProcess() })?)
    }

    /// The binary SID of the user a process runs as.
    fn user_sid(process: HANDLE) -> std::io::Result<Vec<u8>> {
        let mut token = HANDLE::default();
        // SAFETY: `process` is a valid process handle; the token handle is owned from here on.
        let token = unsafe {
            OpenProcessToken(process, TOKEN_QUERY, &mut token)?;
            OwnedHandle::from_raw_handle(token.0)
        };
        // 8-byte aligned and far larger than a TOKEN_USER with the longest possible SID.
        let mut buf = [0u64; 64];
        let mut len = 0u32;
        // SAFETY: the buffer size passed is its real size.
        unsafe {
            GetTokenInformation(
                HANDLE(token.as_raw_handle()),
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                size_of_val(&buf) as u32,
                &mut len,
            )?
        };
        // SAFETY: GetTokenInformation filled the buffer with a TOKEN_USER whose SID lives inside it.
        let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        // SAFETY: `sid` points at a valid SID, GetLengthSid gives its size.
        Ok(unsafe { std::slice::from_raw_parts(sid.0.cast::<u8>(), GetLengthSid(sid) as usize) }.to_vec())
    }

    /// A random per-user pipe name suffix kept in the data directory, so the name cannot be squatted before the app
    /// first runs. The security does not rest on it: the DACL and the client's identity check do. A missing, empty
    /// or corrupt file is replaced.
    pub fn endpoint_key() -> std::io::Result<String> {
        use std::hash::{BuildHasher, Hasher};

        let dir = snap_core::paths::data_dir();
        let path = dir.join("ipc.key");
        let read = || std::fs::read_to_string(&path).map(|k| k.trim().to_owned()).ok().filter(|k| k.len() == 32);
        if let Some(key) = read() {
            return Ok(key);
        }
        // Each `RandomState` is seeded from the OS, so two of them give 128 unpredictable bits.
        let bits = || std::hash::RandomState::new().build_hasher().finish();
        let key = format!("{:016x}{:016x}", bits(), bits());
        std::fs::create_dir_all(&dir)?;
        // Written whole under a private name first, so a reader never sees a half-written key.
        let staged = dir.join(format!("ipc.key.{}", std::process::id()));
        std::fs::write(&staged, &key)?;
        // Linking fails when the file exists. A valid key there came from a concurrent starter and wins; anything
        // else (an empty or corrupt file, a volume without hard links) is overwritten.
        let result = match std::fs::hard_link(&staged, &path) {
            Ok(()) => Ok(key),
            Err(_) => match read() {
                Some(winner) => Ok(winner),
                None => std::fs::rename(&staged, &path).map(|()| key),
            },
        };
        let _ = std::fs::remove_file(&staged);
        result
    }
}

#[cfg(unix)]
fn socket_name() -> Result<Name<'static>> {
    use interprocess::local_socket::{GenericFilePath, ToFsName};
    socket_path().map_err(Error::Io)?.to_fs_name::<GenericFilePath>().map_err(Error::Io)
}

/// `$XDG_RUNTIME_DIR/snapoleon.sock`, or inside a private (0700) temp directory where there is no runtime dir
/// (macOS temp dirs are already per-user). A temp directory that someone else created first is refused: its owner
/// could swap our socket for theirs.
#[cfg(unix)]
fn socket_path() -> std::io::Result<std::path::PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).filter(|p| p.is_dir()) {
        return Ok(dir.join("snapoleon.sock"));
    }
    // SAFETY: geteuid takes no arguments and cannot fail.
    let uid = unsafe { geteuid() };
    let dir = std::env::temp_dir().join(format!("snapoleon-{uid}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(&dir)?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is not a private directory owned by this user", dir.display()),
        ));
    }
    Ok(dir.join("snapoleon.sock"))
}
