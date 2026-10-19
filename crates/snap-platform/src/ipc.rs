//! Single-instance IPC.
//!
//! Transport is a local socket (named pipe `\\.\pipe\snapoleon-<user>` on
//! Windows, a Unix domain socket in `$XDG_RUNTIME_DIR` or a private temp
//! directory elsewhere) carrying one newline-delimited JSON
//! [`Request`] and one [`Response`] per connection.
//!
//! [`serve`] doubles as the single-instance check: it fails with
//! [`AlreadyRunning`] when another process already owns the endpoint.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use interprocess::ConnectWaitMode;
use interprocess::local_socket::traits::Listener as _;
use interprocess::local_socket::{ConnectOptions, Listener, ListenerOptions, Name, Stream};
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
        Ok(req) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(req))).unwrap_or_else(|_| Response::err("request handler panicked")),
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
    let err = match ListenerOptions::new().name(name.clone()).create_sync() {
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
    reclaim_stale(name)
}

#[cfg(unix)]
fn reclaim_stale(name: Name<'static>) -> Result<Listener> {
    let _ = std::fs::remove_file(socket_path());
    ListenerOptions::new().name(name).create_sync().map_err(Error::Io)
}

#[cfg(not(unix))]
fn reclaim_stale(_name: Name<'static>) -> Result<Listener> {
    Err(Error::Other(anyhow::Error::new(AlreadyRunning)))
}

fn connect_once() -> std::io::Result<Stream> {
    ConnectOptions::new().name(socket_name().map_err(|e| std::io::Error::other(e.to_string()))?).connect_sync()
}

/// Connect, retrying briefly while the server is between pipe instances.
fn connect(timeout: Duration) -> std::io::Result<Stream> {
    let name = socket_name().map_err(|e| std::io::Error::other(e.to_string()))?;
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match ConnectOptions::new().name(name.clone()).wait_mode(ConnectWaitMode::Timeout(left)).connect_sync() {
            Ok(s) => return Ok(s),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => return Err(e),
            Err(e) if left.is_zero() => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

#[cfg(windows)]
fn socket_name() -> Result<Name<'static>> {
    use interprocess::local_socket::{GenericNamespaced, ToNsName};
    format!("snapoleon-{}", crate::util::user_name()).to_ns_name::<GenericNamespaced>().map_err(Error::Io)
}

#[cfg(unix)]
fn socket_name() -> Result<Name<'static>> {
    use interprocess::local_socket::{GenericFilePath, ToFsName};
    socket_path().to_fs_name::<GenericFilePath>().map_err(Error::Io)
}

/// `$XDG_RUNTIME_DIR/snapoleon.sock`, or inside a private (0700) temp directory
/// where there is no runtime dir (macOS temp dirs are already per-user).
#[cfg(unix)]
fn socket_path() -> std::path::PathBuf {
    use std::os::unix::fs::DirBuilderExt;

    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).filter(|p| p.is_dir()) {
        return dir.join("snapoleon.sock");
    }
    let dir = std::env::temp_dir().join(format!("snapoleon-{}", crate::util::user_name()));
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir);
    dir.join("snapoleon.sock")
}
