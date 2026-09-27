//! The IPC link to the core process.
//!
//! The GUI is the socket *server* even though it is the one issuing calls: `RunCore` in
//! The core's `main.go` reads `NUNYA_CORE_SOCKET` and dials in. So the startup order is bind, spawn,
//! accept — never connect.
//!
//! On Linux and macOS the link is a unix socket; on Windows it is a named pipe, which is what the
//! core dials there (`ConnectIPC` in its `internal/ipc/ipc_windows.go`). Everything past the accept
//! (framing, the call table, teardown) is the same stream either way.
//!
//! One accepted connection at a time. When the core exits the read loop ends, every in-flight call
//! is failed rather than left hanging, and the listener goes back to waiting for the next core.

use std::collections::HashMap;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{oneshot, Mutex};

/// Bound before the async runtime exists, so it stays a std listener until `accept_loop` adopts it.
#[cfg(unix)]
pub type PendingListener = std::os::unix::net::UnixListener;

/// The pipe's name. tokio creates a pipe only on a runtime, and `bind` runs outside one, so the
/// first instance is made by `accept_loop`. The core does not notice the gap: it retries its dial
/// for five seconds (`RunCore` in the core's `main.go`).
#[cfg(windows)]
pub type PendingListener = std::ffi::OsString;

/// The write half of whichever stream the platform accepted.
type Writer = Box<dyn AsyncWrite + Send + Unpin>;

use super::codec::{self, Response};
use super::peer;

/// A call that outlives this is a hung core, not a slow one. `Test` sweeps are the long pole and
/// they carry their own timeout well under this.
const CALL_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("the core is not connected")]
    NotConnected,
    #[error("the core did not answer {method} within {}s", CALL_TIMEOUT.as_secs())]
    Timeout { method: String },
    #[error("core returned an error: {0}")]
    Core(String),
    #[error("could not decode the core's reply: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("ipc failure: {0}")]
    Io(#[from] io::Error),
}

type Pending = Arc<Mutex<HashMap<u32, oneshot::Sender<Response>>>>;

pub struct CoreLink {
    socket_path: PathBuf,
    writer: Arc<Mutex<Option<Writer>>>,
    pending: Pending,
    next_id: AtomicU32,
    /// Pid of the core we spawned. Set before the child can possibly connect, because
    /// `Command::spawn` hands back the pid before the child execs.
    expected_pid: Arc<AtomicU32>,
}

impl CoreLink {
    /// Binds the socket. The parent directory is expected to be private already; the socket itself
    /// is narrowed to owner-only so a stranger cannot even attempt a connection.
    ///
    /// Returns a *std* listener rather than a tokio one: this runs from Tauri's `setup`, which is
    /// outside any runtime, and tokio's socket constructors panic without a reactor to register
    /// with. `accept_loop` adopts it once it is on the runtime.
    ///
    /// On Windows `socket_path` only names the directory the core works in. The link is a pipe, and
    /// `socket_path()` answers with the pipe's name, which is what the core must be given.
    pub fn bind(socket_path: impl Into<PathBuf>) -> io::Result<(Arc<Self>, PendingListener)> {
        let socket_path = socket_path.into();
        if let Some(dir) = socket_path.parent() {
            std::fs::create_dir_all(dir)?;
            #[cfg(unix)]
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }

        #[cfg(unix)]
        let listener = {
            // A crashed run leaves the socket file behind and bind() would fail with EADDRINUSE.
            if socket_path.exists() {
                std::fs::remove_file(&socket_path)?;
            }
            let listener = PendingListener::bind(&socket_path)?;
            // Required before tokio can adopt it; a blocking accept would stall the runtime thread.
            listener.set_nonblocking(true)?;
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
            listener
        };

        #[cfg(windows)]
        let (socket_path, listener) = {
            let name = pipe_name();
            (PathBuf::from(&name), name)
        };



        let link = Arc::new(Self {
            socket_path,
            writer: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU32::new(1),
            expected_pid: Arc::new(AtomicU32::new(0)),
        });

        Ok((link, listener))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Records which process is allowed to connect. Call this immediately after spawning the core.
    pub fn expect_core_pid(&self, pid: u32) {
        self.expected_pid.store(pid, Ordering::SeqCst);
    }

    pub async fn is_connected(&self) -> bool {
        self.writer.lock().await.is_some()
    }

    /// Accepts core connections forever. Each accepted socket is verified, served until EOF, then
    /// dropped so the next core process can take its place.
    #[cfg(unix)]
    pub async fn accept_loop(
        self: Arc<Self>,
        listener: PendingListener,
        on_change: impl Fn(bool) + Send + Sync + 'static,
    ) {
        use std::os::unix::io::AsRawFd;

        let listener = match tokio::net::UnixListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => {
                log::error!("could not adopt the core listener onto the runtime: {e}");
                return;
            }
        };
        let on_change = Arc::new(on_change);

        loop {
            let (stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(e) => {
                    log::error!("accept failed, the core link is dead: {e}");
                    return;
                }
            };

            let expected = self.expected_pid.load(Ordering::SeqCst);
            if let Err(why) = peer::verify(stream.as_raw_fd(), expected) {
                log::warn!("rejected a connection to the core socket: {why}");
                drop(stream);
                continue;
            }

            log::info!("core connected (pid {expected})");
            self.clone().serve_connection(stream, on_change.clone()).await;
            log::info!("core disconnected");
        }
    }

    /// The pipe version of the same loop. A pipe instance carries one connection, so the next one
    /// is created before the connected one is served, and a core restarted meanwhile finds it.
    ///
    /// `first_pipe_instance` makes creating the pipe fail if something else already made one of
    /// this name, rather than joining it, so the pipe that is served is always ours.
    #[cfg(windows)]
    pub async fn accept_loop(
        self: Arc<Self>,
        listener: PendingListener,
        on_change: impl Fn(bool) + Send + Sync + 'static,
    ) {
        use tokio::net::windows::named_pipe::ServerOptions;

        let mut server = match ServerOptions::new().first_pipe_instance(true).create(&listener) {
            Ok(s) => s,
            Err(e) => {
                log::error!("could not create the core pipe: {e}");
                return;
            }
        };
        let on_change = Arc::new(on_change);

        loop {
            if let Err(e) = server.connect().await {
                log::error!("accept failed, the core link is dead: {e}");
                return;
            }
            let next = match ServerOptions::new().create(&listener) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("could not create the next core pipe: {e}");
                    return;
                }
            };
            let stream = std::mem::replace(&mut server, next);

            let expected = self.expected_pid.load(Ordering::SeqCst);
            if let Err(why) = peer::verify_pipe(&stream, expected) {
                log::warn!("rejected a connection to the core pipe: {why}");
                drop(stream);
                continue;
            }

            log::info!("core connected (pid {expected})");
            self.clone().serve_connection(stream, on_change.clone()).await;
            log::info!("core disconnected");
        }
    }

    async fn serve_connection<S>(
        self: Arc<Self>,
        stream: S,
        on_change: Arc<impl Fn(bool) + Send + Sync + 'static>,
    ) where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (mut read_half, write_half) = tokio::io::split(stream);
        *self.writer.lock().await = Some(Box::new(write_half));
        on_change(true);

        loop {
            match codec::read_response(&mut read_half).await {
                Ok(resp) => {
                    let waiter = self.pending.lock().await.remove(&resp.id);
                    match waiter {
                        // A call that already timed out leaves no receiver; dropping is correct.
                        Some(tx) => {
                            let _ = tx.send(resp);
                        }
                        None => log::debug!("reply {} had no waiter, discarding", resp.id),
                    }
                }
                Err(e) => {
                    if e.kind() != io::ErrorKind::UnexpectedEof {
                        log::error!("core link read failed: {e}");
                    }
                    break;
                }
            }
        }

        // Tear down before waking anyone, so a caller that retries immediately sees NotConnected
        // instead of writing into a half-closed socket.
        if let Some(mut w) = self.writer.lock().await.take() {
            let _ = w.shutdown().await;
        }
        self.fail_all_pending().await;
        on_change(false);
    }

    async fn fail_all_pending(&self) {
        let mut pending = self.pending.lock().await;
        if !pending.is_empty() {
            log::warn!("failing {} in-flight calls: core is gone", pending.len());
        }
        // Dropping each sender makes the awaiting receiver resolve to Err, which `call` maps to
        // NotConnected. Leaving them in place would hang every caller until the timeout.
        pending.clear();
    }

    /// Issues one request and waits for its reply.
    pub async fn call_raw(&self, method: &str, payload: Vec<u8>) -> Result<Vec<u8>, LinkError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let frame = codec::encode_request(id, method, &payload);

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        // Scope the writer lock to the write itself: a slow socket must not serialise every caller
        // for the whole round trip.
        {
            let mut guard = self.writer.lock().await;
            let writer = match guard.as_mut() {
                Some(w) => w,
                None => {
                    self.pending.lock().await.remove(&id);
                    return Err(LinkError::NotConnected);
                }
            };
            if let Err(e) = codec::write_all(writer, &frame).await {
                self.pending.lock().await.remove(&id);
                return Err(LinkError::Io(e));
            }
        }

        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(resp)) => resp.into_result().map_err(LinkError::Core),
            // Sender dropped: the read loop tore down while we waited.
            Ok(Err(_)) => Err(LinkError::NotConnected),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(LinkError::Timeout {
                    method: method.to_string(),
                })
            }
        }
    }

    /// Typed call. Encodes a prost message, decodes the reply into another.
    pub async fn call<Req, Resp>(&self, method: &str, req: &Req) -> Result<Resp, LinkError>
    where
        Req: prost::Message,
        Resp: prost::Message + Default,
    {
        let bytes = self.call_raw(method, req.encode_to_vec()).await?;
        Ok(Resp::decode(bytes.as_slice())?)
    }
}

impl Drop for CoreLink {
    fn drop(&mut self) {
        // Leaving the socket file behind would make the next run's bind() fail. A pipe has no file;
        // it goes when its last handle closes.
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// A pipe name no other link will use: the main core's and every probe core's are bound from one
/// process, and a name reused from a crashed run would fail `first_pipe_instance`.
#[cfg(windows)]
fn pipe_name() -> std::ffi::OsString {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        r"\\.\pipe\nunya-{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
    .into()
}
