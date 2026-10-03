//! What macOS, Linux and Windows do the same way, re-exported by each of their files.
//!
//! **How the core is hosted.** On a desktop the core is a child process that dials back into a
//! socket (a pipe on Windows) the app binds first: bind, spawn, accept (`rpc/link.rs`). The core
//! checks that its parent is `Nunya`, and `rpc/peer.rs` checks who dialled in, so a local process
//! that wins the race to the socket cannot pose as the core. A phone cannot start programs like
//! this, and hosts the core in-process instead (`mobile.rs`), which is why `Engine` is a platform
//! item: everything above it only calls the core and never asks how it is reached.
//!
//! Also here: bringing the window back, which only a desktop window can be un-minimised for,
//! handing a page to the system browser, which a desktop does by running its own opener, and
//! remembering that a tray could not be made, which every desktop tray asks.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

use crate::core_proc::{self, CoreProcess};
use crate::rpc::{CoreLink, LinkError};

/// A scratch core that has not dialled in by now is not going to.
const SCRATCH_STARTUP: Duration = Duration::from_secs(10);

/// One running core and the link to it.
pub struct Engine {
    link: Arc<CoreLink>,
    process: Mutex<Option<CoreProcess>>,
    /// The private directory holding the socket, removed on `shut_down`.
    dir: PathBuf,
    binary: PathBuf,
    /// The window to forward the log to. Only the app's own core has one; a scratch core logs at
    /// debug level and nowhere else.
    app: Option<AppHandle>,
}

impl Engine {
    /// Starts the app's own core, at launch.
    ///
    /// Runs from Tauri's `setup`, outside the async runtime, but tokio's process machinery
    /// registers a SIGCHLD handler with the reactor and the log pump calls `tokio::spawn`, so the
    /// spawn is entered on the runtime even though the call itself is not async. The window hears
    /// `core-connection` whenever the core dials in or goes away.
    pub fn launch(app: &AppHandle) -> Result<Arc<Engine>, String> {
        // Per-user private directory. On macOS `temp_dir` is already inside the user's sandboxed
        // `/var/folders/...` tree, which keeps the socket path well under the 104-byte `sun_path`
        // limit.
        let dir = std::env::temp_dir().join(format!("nunya-{}", std::process::id()));
        let (link, listener) =
            CoreLink::bind(dir.join("core.sock")).map_err(|e| format!("could not bind the core socket: {e}"))?;
        log::info!("core socket at {}", link.socket_path().display());

        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let binary = core_proc::find_core(exe.parent().ok_or("executable has no parent directory")?);
        let engine = Arc::new(Engine {
            link: link.clone(),
            process: Mutex::new(None),
            dir,
            binary,
            app: Some(app.clone()),
        });
        let pid = tauri::async_runtime::block_on(engine.restart())?;
        log::info!("core started (pid {pid})");

        let notify = app.clone();
        tauri::async_runtime::spawn(async move {
            link.accept_loop(listener, move |connected| {
                let _ = notify.emit("core-connection", connected);
            })
            .await;
        });
        Ok(engine)
    }

    /// A short-lived core of its own, for one probe run: its own directory and socket, so it
    /// cannot collide with the app's core, and nothing told to the window.
    pub async fn scratch(&self) -> Result<Engine, String> {
        Engine::start(self.binary.clone()).await
    }

    /// A core on `binary` with no window: what a probe runs, and what the integration tests start.
    /// Returns once the core has dialled in.
    pub async fn start(binary: PathBuf) -> Result<Engine, String> {
        let dir = std::env::temp_dir().join(format!(
            "nunya-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let (link, listener) = CoreLink::bind(dir.join("core.sock"))
            .map_err(|e| format!("could not bind the probe socket: {e}"))?;
        let engine = Engine {
            link: link.clone(),
            process: Mutex::new(None),
            dir,
            binary,
            app: None,
        };
        if let Err(e) = engine.restart().await {
            engine.shut_down().await;
            return Err(e);
        }
        tokio::spawn(async move { link.accept_loop(listener, |_| {}).await });

        let deadline = tokio::time::Instant::now() + SCRATCH_STARTUP;
        while !engine.is_connected().await {
            if tokio::time::Instant::now() >= deadline {
                engine.shut_down().await;
                return Err("the probe core never connected".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(engine)
    }

    /// One call to the core: a protobuf request, a protobuf reply (`rpc::method` names them).
    pub async fn call<Req, Resp>(&self, method: &str, req: &Req) -> Result<Resp, LinkError>
    where
        Req: prost::Message,
        Resp: prost::Message + Default,
    {
        self.link.call(method, req).await
    }

    pub async fn is_connected(&self) -> bool {
        self.link.is_connected().await
    }

    /// Starts a core in place of the one running, if any — at launch, after the grant gave the
    /// binary its privilege, and after a failed update. Returns its pid.
    pub async fn restart(&self) -> Result<u32, String> {
        let mut slot = self.process.lock().await;
        if let Some(old) = slot.take() {
            old.stop().await;
        }
        let app = self.app.clone();
        let core = CoreProcess::spawn(&self.binary, self.link.socket_path(), &self.dir, move |line| {
            log::debug!("core: {line}");
            if let Some(app) = &app {
                let _ = app.emit("core-log", line);
            }
        })
        .map_err(|e| e.to_string())?;
        // Before the first accept, so the peer check has something to compare against. The kernel
        // queues the core's connection in the listen backlog meanwhile.
        self.link.expect_core_pid(core.pid);
        let pid = core.pid;
        *slot = Some(core);
        Ok(pid)
    }

    /// Stops the core process, keeping the link for the next one.
    pub async fn stop(&self) {
        if let Some(core) = self.process.lock().await.take() {
            core.stop().await;
        }
    }

    /// Stops the core and removes its directory. A core left running would keep the TUN and its
    /// routes installed, or a scratch core's proxy ports open on loopback.
    pub async fn shut_down(&self) {
        self.stop().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }

    /// The core binary, which is what the VPN-mode grant gives its privilege to.
    pub fn binary(&self) -> &Path {
        &self.binary
    }
}

/// Brings the main window back from hidden or minimised, and to the front.
pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Nothing to register: a desktop has no native half beside the Rust one.
pub fn native_setup(
    _app: &AppHandle,
    _api: tauri::plugin::PluginApi<tauri::Wry, ()>,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

/// Runs the desktop's own opener, and waits only for it to hand the page off.
pub fn open_with(mut opener: Command) -> Result<(), String> {
    let status = opener
        .status()
        .map_err(|e| format!("could not start the system browser: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the system browser could not be opened ({status})"))
    }
}

/// Set once the tray has been found not to work here, so it is not attempted again.
static NO_TRAY: OnceLock<()> = OnceLock::new();

/// Whether a tray is worth attempting at all.
///
/// On Linux this used to dlopen libayatana, which tray-icon loads lazily and which panics when it is
/// missing. Nothing loads it any more — `platform/sni.rs` speaks the protocol directly — so the
/// only real question is whether a watcher answers, which installing finds out by asking. A failure
/// is remembered rather than retried: the frontend reports a status every few seconds, and each
/// attempt would wait on the bus before failing the same way.
pub fn supported() -> bool {
    NO_TRAY.get().is_none()
}

/// Remembers that there is no tray here, so `set_tray_status` stops trying.
pub fn give_up() {
    let _ = NO_TRAY.set(());
}
