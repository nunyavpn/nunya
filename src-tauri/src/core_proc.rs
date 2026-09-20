//! Spawning and supervising the core process.
//!
//! The core is a child process that dials back into our socket. It refuses to run if its parent is
//! not who it expects (`CheckParentProcess` in the core's `internal/parentcheck`), which in release
//! builds means the parent binary must be named `Nunya` and sit in the same directory as the core.
//! Development builds of the core use the `noparentcheck` tag to lift that.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Environment variable the core reads to find our socket (`RunCore` in the core's `main.go`).
const SOCKET_ENV: &str = "NUNYA_CORE_SOCKET";

/// Turns an unrecovered Go panic into a real abort so every goroutine stack is dumped, rather than
/// the single-goroutine default. Matches what the Qt build sets.
const TRACEBACK_ENV: &str = "GOTRACEBACK";

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("core binary not found at {0}")]
    NotFound(PathBuf),
    #[error("could not start the core: {0}")]
    Io(#[from] std::io::Error),
}

pub struct CoreProcess {
    child: Arc<Mutex<Option<Child>>>,
    pub pid: u32,
}

impl CoreProcess {
    /// Starts the core pointed at `socket_path`.
    ///
    /// `on_log` receives every line the core writes to stdout or stderr. The core logs its
    /// sing-box and Xray versions on startup and prints "Core Has Successfully Connected to
    /// Nunya!" once the IPC link is up, which is the readiness signal worth surfacing.
    pub fn spawn(
        core_path: &Path,
        socket_path: &Path,
        asset_dir: &Path,
        on_log: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<Self, SpawnError> {
        if !core_path.exists() {
            return Err(SpawnError::NotFound(core_path.to_path_buf()));
        }

        let mut cmd = Command::new(core_path);
        cmd.env(SOCKET_ENV, socket_path)
            .env(TRACEBACK_ENV, "crash")
            // Points Xray's asset loader at a writable directory so a geoip/geosite file dropped
            // there later is found without restarting the core.
            .env("XRAY_LOCATION_ASSET", asset_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);

        let on_log = Arc::new(on_log);
        if let Some(out) = child.stdout.take() {
            pump(out, on_log.clone());
        }
        if let Some(err) = child.stderr.take() {
            pump(err, on_log);
        }

        Ok(Self {
            child: Arc::new(Mutex::new(Some(child))),
            pid,
        })
    }

    /// Terminates the core and reaps it.
    pub async fn stop(&self) {
        let mut guard = self.child.lock().await;
        if let Some(mut child) = guard.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    /// Waits for the core to exit and reports how. `Some(code)` is a normal exit; the core's own
    /// panic handler uses code 2, which is how a Go panic is told from a clean stop.
    pub async fn wait(&self) -> Option<i32> {
        let mut guard = self.child.lock().await;
        let child = guard.as_mut()?;
        child.wait().await.ok().and_then(|s| s.code())
    }
}

fn pump<R>(reader: R, on_log: Arc<impl Fn(String) + Send + Sync + 'static>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            on_log(line);
        }
    });
}

/// Locates the core binary.
///
/// In a bundled app it sits beside the executable, which is also what the core's own parent check
/// requires. `NUNYA_CORE_PATH` overrides it for development, where the core is built into a
/// scratch directory rather than copied into the bundle.
pub fn find_core(exe_dir: &Path) -> PathBuf {
    if let Ok(explicit) = std::env::var("NUNYA_CORE_PATH") {
        return PathBuf::from(explicit);
    }
    exe_dir.join("nunya-core")
}
