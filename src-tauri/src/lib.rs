//! Application library. `main.rs` is a shim over `run()` so that integration tests can use
//! the same modules the binary does.

pub mod config;
pub mod core_proc;
pub mod external;
pub mod geo;
pub mod rpc;
pub mod storage;
pub mod subscription;
pub mod sysproxy;
pub mod transport;
mod tray;

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{Emitter, Manager, State};
use tokio::sync::Mutex;

use config::BuildRequest;
use core_proc::CoreProcess;
use rpc::gen;
use rpc::{method, CoreLink};
use transport::{Throughput, TunnelState, TunnelTransport};

struct AppState {
    link: Arc<CoreLink>,
    core: Mutex<Option<CoreProcess>>,
    runtime_dir: PathBuf,
    /// Where the core binary is, so a probe can start one of its own.
    core_path: PathBuf,
    /// How the tunnel is actually carried. Chosen at startup; see `transport::select`.
    tunnel: Arc<dyn TunnelTransport>,
    tunnel_kind: transport::select::Kind,
    /// The system proxy as it was before this app set it, while it is set. Also on disk, in
    /// `sysproxy::SAVED_FILE`, so a crash cannot strand it.
    system_proxy: Arc<std::sync::Mutex<Option<sysproxy::Saved>>>,
}

/// Puts the system proxy back if this app set it: the settings from before, then the file.
///
/// Blocking — it runs the desktop's own tools — and safe to call when nothing was set, which is
/// what lets every way out (disconnect, quit, a dead core) call it without keeping score.
fn release_system_proxy(dir: &std::path::Path, slot: &std::sync::Mutex<Option<sysproxy::Saved>>) -> Result<(), String> {
    let Some(saved) = slot.lock().unwrap_or_else(|p| p.into_inner()).take() else {
        return Ok(());
    };
    let result = sysproxy::restore(&saved);
    match &result {
        Ok(()) => {
            sysproxy::remove_saved(dir);
            log::info!("system proxy restored");
        }
        // The file stays, so the next launch tries again rather than leaving it pointed at us.
        Err(e) => log::error!("could not restore the system proxy: {e}"),
    }
    result
}

/// The core reports errors two different ways: a transport failure comes back as a `LinkError`,
/// while a handler that ran but refused comes back as `ErrorResp.error` with a status of OK. Both
/// are failures as far as the UI is concerned, so they collapse here.
fn or_err(resp: gen::ErrorResp) -> Result<(), String> {
    match resp.error {
        Some(e) if !e.is_empty() => Err(e),
        _ => Ok(()),
    }
}

/// What the UI needs to decide whether to offer a connect button, and what to say if not.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Readiness {
    /// Which mode this answer is about, so a stale reply cannot be read as being about the other.
    mode: &'static str,
    /// Which transport is carrying the tunnel, so the UI never has to guess.
    transport: &'static str,
    ready: bool,
    state: TunnelState,
    /// Present when the transport cannot run: an unprivileged core, or an unapproved VPN profile.
    detail: Option<String>,
}

#[tauri::command]
async fn core_connected(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.link.is_connected().await)
}

/// Whether the tunnel can actually be brought up, and if not, why.
///
/// Asked before the user is offered a connect button, so an unprivileged core or an unapproved VPN
/// profile is stated up front rather than discovered as a failure afterwards.
///
/// The mode is the caller's, because readiness is a different question for each: VPN mode needs
/// privilege or a system approval, proxy mode needs a free port and nothing else.
#[tauri::command]
async fn tunnel_readiness(
    state: State<'_, AppState>,
    mode: Option<config::Mode>,
) -> Result<Readiness, String> {
    let mode = mode.unwrap_or_default();
    let (state_or_err, detail) = match state.tunnel.availability(mode).await {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(e.to_string())),
    };

    Ok(Readiness {
        mode: mode.as_str(),
        transport: state.tunnel_kind.as_str(),
        ready: matches!(state_or_err, Some(TunnelState::Disconnected)),
        state: state_or_err.unwrap_or(TunnelState::NeedsPermission),
        detail,
    })
}

/// Builds the config and asks the core to parse it, without starting anything.
///
/// Returns the generated JSON so it can be inspected; a config that cannot be explained is a
/// config nobody can trust.
#[tauri::command]
async fn check_config(state: State<'_, AppState>, req: BuildRequest) -> Result<String, String> {
    let cfg = config::build(&req);
    let pretty = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;

    let resp: gen::ErrorResp = state
        .link
        .call(
            method::CHECK_CONFIG,
            &gen::LoadConfigReq {
                core_config: Some(cfg.to_string()),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    or_err(resp)?;
    Ok(pretty)
}

/// Asks for whatever consent the platform requires, once.
///
/// A no-op for the subprocess transport, which gets its privilege from how the core was installed.
#[tauri::command]
async fn request_permission(state: State<'_, AppState>) -> Result<(), String> {
    state
        .tunnel
        .request_permission()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_tunnel(state: State<'_, AppState>, req: BuildRequest) -> Result<(), String> {
    state.tunnel.start(&req).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn stop_tunnel(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let stopped = state.tunnel.stop().await.map_err(|e| e.to_string());
    // Here as well as in the frontend, so no path that stops the tunnel can leave the system
    // proxy pointing at a listener that is gone.
    let dir = data_dir(&app)?;
    let slot = state.system_proxy.clone();
    let _ = tokio::task::spawn_blocking(move || release_system_proxy(&dir, &slot)).await;
    stopped
}

/// Points the system proxy at the proxy-mode listener on `port`.
///
/// The settings it replaces are captured and written to disk first, and only once — a second
/// call while ours are applied must not capture ours as "the user's" and restore them later. If
/// applying fails part-way, what was captured is put straight back.
#[tauri::command]
async fn set_system_proxy(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    port: u16,
) -> Result<(), String> {
    let dir = data_dir(&app)?;
    let slot = state.system_proxy.clone();
    tokio::task::spawn_blocking(move || {
        {
            let mut held = slot.lock().unwrap_or_else(|p| p.into_inner());
            if held.is_none() {
                let saved = sysproxy::capture()?;
                sysproxy::write_saved(&dir, &saved)?;
                *held = Some(saved);
            }
        }
        if let Err(e) = sysproxy::apply(port) {
            let _ = release_system_proxy(&dir, &slot);
            return Err(e);
        }
        log::info!("system proxy set to 127.0.0.1:{port}");
        Ok(())
    })
    .await
    .map_err(|e| format!("system proxy task panicked: {e}"))?
}

/// Restores the system proxy this app set, if it set one.
#[tauri::command]
async fn clear_system_proxy(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let dir = data_dir(&app)?;
    let slot = state.system_proxy.clone();
    tokio::task::spawn_blocking(move || release_system_proxy(&dir, &slot))
        .await
        .map_err(|e| format!("system proxy task panicked: {e}"))?
}

#[tauri::command]
async fn query_stats(state: State<'_, AppState>) -> Result<Throughput, String> {
    state.tunnel.throughput().await.map_err(|e| e.to_string())
}

/// One server's measurement, keyed back to the caller's ordering.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LatencyResult {
    /// Index into the profiles that were submitted.
    index: usize,
    /// Milliseconds, or -1 when the server did not answer.
    latency_ms: i32,
    /// Why it failed, when it did. Kept so a user can tell a blocked server from a dead one.
    error: Option<String>,
}

/// Endpoints the latency test measures against, in order.
///
/// No single URL reaches every server, and the two failure modes are structural rather than
/// flaky. A Cloudflare Workers proxy — which is what most free VLESS and Trojan subscriptions
/// are — cannot make a subrequest to Cloudflare's own addresses, so it can never reach
/// `cp.cloudflare.com`. A WARP endpoint egresses somewhere that often cannot reach Google, so
/// `gstatic.com` fails for exactly the servers a Cloudflare endpoint would suit.
///
/// Measured against a real subscription: `cp.cloudflare.com` timed out for all eight Workers
/// servers while both WARP endpoints answered, and `gstatic.com` was the precise inverse. A
/// server is therefore retried against the next endpoint before it is called unreachable, and
/// the first entry is the one that answered for every server tested.
const TEST_URLS: [&str; 3] = [
    "http://detectportal.firefox.com/success.txt",
    "http://cp.cloudflare.com/",
    "http://www.gstatic.com/generate_204",
];

/// Measures how long each server takes to reach a known URL.
///
/// This is what Quick Connect ranks on, and what the bars and colours in the list mean. The test
/// runs in its own short-lived core instance with no TUN, so it never disturbs a running tunnel or
/// the system's routing.
///
/// Servers that fail are retried against the next endpoint in `TEST_URLS`, because a failure says
/// as much about the endpoint as about the server. Only a server that fails all of them is
/// reported unreachable. A caller that names its own `url` gets exactly that one: an explicit
/// choice is not second-guessed.
#[tauri::command]
async fn test_servers(
    state: State<'_, AppState>,
    profiles: Vec<config::Profile>,
    url: Option<String>,
    timeout_ms: Option<i32>,
    concurrency: Option<i32>,
) -> Result<Vec<LatencyResult>, String> {
    if profiles.is_empty() {
        return Ok(Vec::new());
    }

    let urls: Vec<String> = match url {
        Some(u) => vec![u],
        None => TEST_URLS.iter().map(|u| u.to_string()).collect(),
    };

    // Indexed by the caller's ordering, so a result always finds its way back to the row that
    // asked for it however many rounds it took.
    let mut results: Vec<Option<LatencyResult>> = (0..profiles.len()).map(|_| None).collect();
    let mut pending: Vec<usize> = (0..profiles.len()).collect();

    for (round, endpoint) in urls.iter().enumerate() {
        if pending.is_empty() {
            break;
        }
        if round > 0 {
            log::info!(
                "retrying {} unreachable server(s) against {endpoint}",
                pending.len()
            );
        }

        let subset: Vec<config::Profile> = pending.iter().map(|&i| profiles[i].clone()).collect();
        let (cfg, tags) = config::build_test(&subset);
        // The subset is re-tagged from t0 each round, so the mapping back to the caller's
        // indices has to be rebuilt with it.
        let by_tag: std::collections::HashMap<String, usize> = tags
            .iter()
            .enumerate()
            .map(|(k, tag)| (tag.clone(), pending[k]))
            .collect();

        let resp: gen::TestResp = state
            .link
            .call(
                method::TEST,
                &gen::TestReq {
                    config: Some(cfg.to_string()),
                    outbound_tags: tags,
                    url: Some(endpoint.clone()),
                    test_timeout_ms: Some(timeout_ms.unwrap_or(5000)),
                    max_concurrency: Some(concurrency.unwrap_or(10)),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| e.to_string())?;

        for item in resp.results {
            let Some(&index) = item.outbound_tag.as_deref().and_then(|t| by_tag.get(t)) else {
                // A tag we did not ask about; nothing sensible to attribute it to.
                continue;
            };
            let error = item.error.filter(|e| !e.is_empty());
            results[index] = Some(LatencyResult {
                index,
                // A failure reports 0ms, which would otherwise sort as the fastest server there is.
                latency_ms: if error.is_some() {
                    -1
                } else {
                    item.latency_ms.unwrap_or(0)
                },
                error,
            });
        }

        // Anything still without a successful measurement goes to the next endpoint, including
        // a tag the core said nothing about at all.
        pending.retain(|&i| !matches!(&results[i], Some(r) if r.error.is_none()));
    }

    Ok(results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.unwrap_or(LatencyResult {
                index,
                latency_ms: -1,
                error: Some("the core reported nothing for this server".to_string()),
            })
        })
        .collect())
}

/// One server's latency, trying `TEST_URLS` in order, as `test_servers` does for a whole list.
async fn latency_of(link: &CoreLink, profile: &config::Profile, timeout_ms: i32) -> (i32, Option<String>) {
    let mut last = String::from("the core reported nothing for this server");
    for url in TEST_URLS {
        let (cfg, tags) = config::build_test(std::slice::from_ref(profile));
        let resp: Result<gen::TestResp, _> = link
            .call(
                method::TEST,
                &gen::TestReq {
                    config: Some(cfg.to_string()),
                    outbound_tags: tags,
                    url: Some(url.to_string()),
                    test_timeout_ms: Some(timeout_ms),
                    max_concurrency: Some(1),
                    ..Default::default()
                },
            )
            .await;
        match resp {
            Ok(resp) => {
                if let Some(item) = resp.results.into_iter().next() {
                    match item.error.filter(|e| !e.is_empty()) {
                        // A failure reports 0ms, which would otherwise read as the fastest server.
                        None => return (item.latency_ms.unwrap_or(0), None),
                        Some(e) => last = e,
                    }
                }
            }
            // The link itself failed; another endpoint will not fix that.
            Err(e) => return (-1, Some(e.to_string())),
        }
    }
    (-1, Some(last))
}

/// One server's result from `check_servers`, sent as the `server-checked` event the moment it is
/// known. `run` ties it to the call that asked, since a row's Check can overlap a Test all.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Checked {
    run: u32,
    index: usize,
    /// Milliseconds, or -1 when the server does not work.
    latency_ms: i32,
    error: Option<String>,
    /// The public address traffic left from — present exactly when traffic went through.
    exit_ip: Option<String>,
    /// Where that is, when a geo service could say.
    exit: Option<geo::Spot>,
}

/// Tests servers end to end, reporting each one as soon as it is done.
///
/// A server works only if traffic goes *through* it and comes out somewhere, so each one is asked
/// for its latency and then for its exit, in one go. Answering the first but not the second — a
/// dial that succeeds and then carries nothing — is a server that does not work, and is reported
/// as such. Placing the exit is separate: a geo service out of quota says nothing about the server.
///
/// One scratch core serves the whole run, with a local port per server; each result is emitted as
/// `server-checked`, so the list fills in row by row instead of waiting for its slowest member.
#[tauri::command]
async fn check_servers(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    run: u32,
    profiles: Vec<config::Profile>,
    timeout_ms: Option<i32>,
) -> Result<(), String> {
    if profiles.is_empty() {
        return Ok(());
    }
    let timeout_ms = timeout_ms.unwrap_or(5000);
    let session = Arc::new(geo::ProbeSession::start(&state.core_path, &profiles).await?);
    let cache = Arc::new(geo::PlaceCache::default());
    let slots = Arc::new(tokio::sync::Semaphore::new(6));

    let mut tasks = Vec::with_capacity(profiles.len());
    for (index, profile) in profiles.into_iter().enumerate() {
        let (app, link, session, cache, slots) = (
            app.clone(),
            state.link.clone(),
            session.clone(),
            cache.clone(),
            slots.clone(),
        );
        tasks.push(tokio::spawn(async move {
            let Ok(_slot) = slots.acquire_owned().await else { return };
            let (mut latency_ms, mut error) = latency_of(&link, &profile, timeout_ms).await;
            let (mut exit_ip, mut exit) = (None, None);

            if latency_ms >= 0 {
                let seen = tokio::task::spawn_blocking(move || {
                    // Once more after a pause: the first request through a fresh port can lose
                    // the race with the server's own handshake.
                    session.exit_of(index).or_else(|_| {
                        std::thread::sleep(std::time::Duration::from_millis(800));
                        session.exit_of(index)
                    }).map(|seen| {
                        let placed = cache.exit(&seen);
                        (seen, placed)
                    })
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                match seen {
                    Ok((seen, placed)) => {
                        exit_ip = Some(seen.plain);
                        exit = placed;
                    }
                    Err(e) => {
                        latency_ms = -1;
                        error = Some(format!("connects, but no traffic came out through it: {e}"));
                    }
                }
            }

            let _ = app.emit(
                "server-checked",
                Checked { run, index, latency_ms, error, exit_ip, exit },
            );
        }));
    }
    for task in tasks {
        let _ = task.await;
    }
    if let Ok(session) = Arc::try_unwrap(session) {
        session.shut_down().await;
    }
    Ok(())
}

/// Finds where each server is: its entry (the address, by DNS) and its exit (measured end to end).
///
/// Separate from `test_servers` because the exit half costs a great deal more: it starts a second
/// core to get a local port per server. The frontend asks for exits only from servers that just
/// answered a latency test. Entries cost one DNS lookup each and are asked first, on their own,
/// so a new row has a real flag within seconds; `entries`/`exits` pick the halves.
#[tauri::command]
async fn locate_servers(
    state: State<'_, AppState>,
    profiles: Vec<config::Profile>,
    entries: Option<bool>,
    exits: Option<bool>,
) -> Result<Vec<geo::Located>, String> {
    geo::locate(
        &state.core_path,
        &profiles,
        entries.unwrap_or(true),
        exits.unwrap_or(true),
    )
    .await
}

/// Where this machine is: its public address and roughly where that is, for the map's first dot.
///
/// Asked directly, so the frontend only asks it with the tunnel down — in VPN mode a direct
/// request goes through the tunnel and would answer for the exit instead.
#[tauri::command]
async fn locate_me() -> Result<geo::Whereabouts, String> {
    tokio::task::spawn_blocking(|| geo::whereabouts(None))
        .await
        .map_err(|e| format!("lookup panicked: {e}"))?
}

/// Where the running tunnel comes out, for the exit chip and the end of the map's route.
///
/// In proxy mode the address is asked through the local listener on `proxy_port`, because only
/// traffic pointed at it goes through the tunnel. In VPN mode everything does, so a direct
/// question already answers for the exit.
#[tauri::command]
async fn locate_exit(proxy_port: Option<u16>) -> Result<geo::Exit, String> {
    tokio::task::spawn_blocking(move || geo::exit_addresses(proxy_port))
        .await
        .map_err(|e| format!("lookup panicked: {e}"))?
}

/// Where the data file lives, and where the socket directory is made.
///
/// Tauri resolves this per platform: `~/Library/Application Support/<identifier>` on macOS.
fn data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("no application data directory: {e}"))
}

/// Reads the saved servers, subscriptions and settings.
///
/// Returns `None` on a first run. A corrupt file is an error rather than a silent reset: losing a
/// server list without saying so is worse than refusing to open with it.
#[tauri::command]
async fn load_data(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let dir = data_dir(&app)?;
    log::info!("reading saved data from {}", storage::data_path(&dir).display());

    let result = tokio::task::spawn_blocking(move || storage::load(&dir))
        .await
        .map_err(|e| format!("load panicked: {e}"))?;

    match &result {
        Ok(Some(text)) => log::info!("loaded {} bytes of saved data", text.len()),
        Ok(None) => log::info!("no saved data yet; starting fresh"),
        Err(e) => log::error!("could not read saved data: {e}"),
    }
    result.map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_data(app: tauri::AppHandle, json: String) -> Result<(), String> {
    let dir = data_dir(&app)?;
    let size = json.len();

    let result = tokio::task::spawn_blocking(move || storage::save(&dir, &json))
        .await
        .map_err(|e| format!("save panicked: {e}"))?;

    match &result {
        Ok(()) => log::debug!("saved {size} bytes"),
        Err(e) => log::error!("could not save data: {e}"),
    }
    result.map_err(|e| e.to_string())
}

/// Fetches a subscription and returns its share links.
///
/// The parsing into profiles stays in the frontend, which already reports rejections line by line
/// for hand-pasted links — a subscription should get the same treatment rather than a silent drop.
#[tauri::command]
async fn fetch_subscription(url: String) -> Result<subscription::Fetched, String> {
    // `fetch` blocks on a network round trip, so it must not run on a runtime worker.
    tokio::task::spawn_blocking(move || subscription::fetch(&url))
        .await
        .map_err(|e| format!("subscription fetch panicked: {e}"))?
        .map_err(|e| e.to_string())
}

/// Opens a page in the system browser — only an `https` page on a host `external` allows.
#[tauri::command]
async fn open_external(url: String) -> Result<(), String> {
    // The opener returns once it has handed the page off, but that is still a process to wait on.
    tokio::task::spawn_blocking(move || external::open(&url))
        .await
        .map_err(|e| format!("opening the page panicked: {e}"))?
}

/// Renders the config without contacting the core, so the UI can show it even when the core is
/// down.
#[tauri::command]
fn preview_config(req: BuildRequest) -> Result<String, String> {
    serde_json::to_string_pretty(&config::build(&req)).map_err(|e| e.to_string())
}

/// Makes closing the window hide it rather than quit — once there is a tray icon to bring it back.
///
/// The tunnel is the point of the app and the window is only its controls, so closing the controls
/// leaves the tunnel as it was; "Quit Nunya" really quits. The tray is created by the frontend's
/// first status (see `tray.rs`), and until it exists closing quits as it always did: hiding a
/// window with no icon to bring it back would leave a tunnel nobody can reach.
fn hide_on_close(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            if !tray::is_up(&handle) {
                return;
            }
            api.prevent_close();
            if let Some(w) = handle.get_webview_window("main") {
                let _ = w.hide();
            }
        }
    });
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .setup(|app| {
            // Per-user private directory. On macOS `temp_dir` is already inside the user's
            // sandboxed `/var/folders/...` tree, which keeps the socket path well under the
            // 104-byte `sun_path` limit.
            let runtime_dir = std::env::temp_dir().join(format!("nunya-{}", std::process::id()));
            let socket = runtime_dir.join("core.sock");

            let (link, listener) = CoreLink::bind(&socket)?;
            log::info!("core socket at {}", link.socket_path().display());

            let exe_dir = std::env::current_exe()?
                .parent()
                .ok_or("executable has no parent directory")?
                .to_path_buf();
            let core_path = core_proc::find_core(&exe_dir);

            let log_sink = app.handle().clone();
            // `setup` runs outside the async runtime, but tokio's process machinery registers a
            // SIGCHLD handler with the reactor and `pump` calls `tokio::spawn`, so this has to be
            // entered on the runtime even though the call itself is not async.
            let core = tauri::async_runtime::block_on(async {
                CoreProcess::spawn(&core_path, &socket, &runtime_dir, move |line| {
                    log::debug!("core: {line}");
                    let _ = log_sink.emit("core-log", line);
                })
            })?;

            // Must happen before the first accept, so the peer check has something to compare
            // against. The kernel queues the core's connection in the listen backlog meanwhile.
            link.expect_core_pid(core.pid);
            log::info!("core started (pid {})", core.pid);

            let notify = app.handle().clone();
            let accept_link = link.clone();
            tauri::async_runtime::spawn(async move {
                accept_link
                    .accept_loop(listener, move |connected| {
                        let _ = notify.emit("core-connection", connected);
                    })
                    .await;
            });

            let tunnel_kind = transport::select::Kind::from_env();
            let tunnel = transport::select::build(tunnel_kind, link.clone());
            log::info!("tunnel transport: {}", tunnel_kind.as_str());

            // A saved system proxy on disk means the last run died holding it. Put it back now,
            // before anything else: until this runs, every browser on the machine is pointed at a
            // port nothing listens on.
            let leftover = data_dir(app.handle())
                .ok()
                .and_then(|dir| sysproxy::read_saved(&dir).map(|saved| (dir, saved)));
            let system_proxy = Arc::new(std::sync::Mutex::new(None));
            if let Some((dir, saved)) = leftover {
                log::warn!("the last session left the system proxy set; restoring it");
                *system_proxy.lock().unwrap_or_else(|p| p.into_inner()) = Some(saved);
                let _ = release_system_proxy(&dir, &system_proxy);
            }

            app.manage(AppState {
                link,
                core_path: core_path.clone(),
                core: Mutex::new(Some(core)),
                runtime_dir,
                tunnel,
                tunnel_kind,
                system_proxy,
            });

            hide_on_close(app.handle());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_connected,
            tunnel_readiness,
            request_permission,
            check_config,
            start_tunnel,
            stop_tunnel,
            query_stats,
            preview_config,
            test_servers,
            locate_servers,
            check_servers,
            locate_me,
            locate_exit,
            set_system_proxy,
            clear_system_proxy,
            fetch_subscription,
            open_external,
            load_data,
            save_data,
            tray::set_tray_status,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the application")
        .run(|app, event| {
            // The Dock icon brings back a window closed to the menu bar, as in any Mac app.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } = event
            {
                tray::show_window(app);
            }
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // A core left running would keep the TUN interface and its routes installed, so
                // the machine would lose connectivity after the UI disappeared.
                let state = app.state::<AppState>();
                if let Ok(dir) = data_dir(app) {
                    let _ = release_system_proxy(&dir, &state.system_proxy);
                }
                tauri::async_runtime::block_on(async {
                    if let Ok(resp) = state
                        .link
                        .call::<_, gen::ErrorResp>(method::STOP, &gen::EmptyReq {})
                        .await
                    {
                        let _ = or_err(resp);
                    }
                    if let Some(core) = state.core.lock().await.as_ref() {
                        core.stop().await;
                    }
                });
                let _ = std::fs::remove_dir_all(&state.runtime_dir);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::TEST_URLS;

    /// The chain exists because one endpoint is never enough, and the order is load-bearing.
    ///
    /// A Cloudflare Workers proxy cannot reach Cloudflare, so leading with `cp.cloudflare.com`
    /// reports every Workers-based server — most of a typical free subscription — as unreachable
    /// while it is working perfectly. That was the original bug; this is the guard against
    /// "simplifying" the list back to one entry or reordering it.
    #[test]
    fn the_fallback_chain_does_not_lead_with_cloudflare() {
        assert!(
            TEST_URLS.len() > 1,
            "a single endpoint cannot measure both Workers proxies and WARP"
        );
        assert!(
            !TEST_URLS[0].contains("cloudflare"),
            "the first endpoint is the one most servers are measured against, and a Cloudflare \
             Workers proxy can never reach Cloudflare"
        );
    }
}
