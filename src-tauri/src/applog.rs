//! The app's own log, where a user can find it: Diagnostics, beside the core's.
//!
//! It used to go to stderr only, which a release build on Windows does not have (it is a GUI
//! program, with no console), so a core that failed to start or a pipe that could not be made left
//! no trace anyone could send us. The first Windows report of a core that would not run was
//! undiagnosable for exactly that reason. stderr still gets everything, as before.
//!
//! What reaches the window is this crate's lines at `info` and above, plus any dependency's
//! warnings and errors; dependencies' `info` is noise there. Lines logged before the window is
//! listening (the core's spawn, the pipe's creation — the ones that matter most when it fails)
//! are kept in a short backlog the frontend fetches once (`app_log_backlog`).

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use log::{Level, Log, Metadata, Record};
use tauri::{AppHandle, Emitter};

/// The event each line is sent as.
pub const EVENT: &str = "app-log";

/// Enough for startup and the first minutes; Diagnostics keeps its own, longer, list from then.
const BACKLOG_MAX: usize = 200;

static APP: OnceLock<AppHandle> = OnceLock::new();
static BACKLOG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

struct Logger(env_logger::Logger);

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.0.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if !self.0.matches(record) {
            return;
        }
        self.0.log(record);
        if record.level() > Level::Warn && !record.target().starts_with("nunya") {
            return;
        }
        let line = format!("{} {}", record.level(), record.args());
        {
            let mut backlog = BACKLOG.lock().unwrap_or_else(|p| p.into_inner());
            if backlog.len() == BACKLOG_MAX {
                backlog.pop_front();
            }
            backlog.push_back(line.clone());
        }
        if let Some(app) = APP.get() {
            let _ = app.emit(EVENT, line);
        }
    }

    fn flush(&self) {
        self.0.flush();
    }
}

/// Installs the logger. `RUST_LOG` still chooses what is logged, defaulting to `info`.
pub fn init() {
    let inner = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).build();
    let max = inner.filter();
    if log::set_boxed_logger(Box::new(Logger(inner))).is_ok() {
        log::set_max_level(max);
    }
}

/// From `setup`, once there is an app to send lines through.
pub fn attach(app: AppHandle) {
    let _ = APP.set(app);
}

/// Everything logged so far that the window may have missed.
pub fn backlog() -> Vec<String> {
    BACKLOG.lock().unwrap_or_else(|p| p.into_inner()).iter().cloned().collect()
}
