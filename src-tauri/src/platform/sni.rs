//! The Linux tray: our own StatusNotifierItem, instead of tray-icon's libayatana one.
//!
//! # Why this exists rather than `TrayIconBuilder`
//!
//! A tray on Linux is a `org.kde.StatusNotifierItem` registered with the shell's watcher; the shell
//! draws it and decides what a click means. tray-icon reaches it through libayatana-appindicator,
//! and two of that library's choices are wrong for this app in ways no call on our side can undo.
//!
//! **The click lag.** GNOME's appindicator extension asks the item, over D-Bus introspection,
//! whether it has an `Activate` method (`supportsActivation` in its `appIndicator.js`). When it
//! does, a left click cannot open the menu straight away: the extension has to wait a full
//! double-click interval (400 ms here) to find out whether a second click is coming, because a
//! double click means `Activate` instead. libayatana always declares `Activate`, and tray-icon's
//! GTK backend never handles it — so every left click paid 400 ms for a method that does nothing,
//! and an impatient second click cancelled the pending menu and called that empty method. The menu
//! looked broken. An item that simply does not declare `Activate` is told apart at introspection
//! and its menu opens on the first click, which is the whole reason this module exists.
//!
//! **The icon race.** libayatana takes an icon as a *file path*, so tray-icon writes a PNG into
//! `$XDG_RUNTIME_DIR` per update and unlinks the previous one. It unlinks before it announces the
//! replacement, so the shell's asynchronous load of the path it was last given can arrive after
//! that path is gone — `Failed to recognize image format`, and a blank icon in the bar. Since the
//! frontend repaints the icon on every status, this fired at nearly every launch. Here the pixels
//! travel over the bus as `IconPixmap`, so there is no file to lose and no order to get wrong.
//!
//! `ItemIsMenu` is true for the same reason the `Activate` method is absent: this item's whole
//! interaction is its menu. Hosts that honour the flag (KDE) then open the menu on a left click
//! too, instead of waiting for an activation that would never be answered.
//!
//! # Shape
//!
//! Everything lives on one thread with its own D-Bus connection, because libdbus is not
//! thread-safe and because a tray must keep answering the shell while the app is busy. The thread
//! owns the [`Item`] state; the app talks to it by sending [`Update`]s down a channel and menu
//! clicks come back up an [`Action`] channel. Nothing here touches Tauri, so `tray.rs` stays the
//! one place that knows what a click should do.
//!
//! The menu is deliberately not a general menu library. It is the five lines `tray.rs` needs, with
//! fixed ids, because a tray menu that could be anything would need a tree, a diffing pass and
//! `LayoutUpdated` revisions to match — all to express a list that has never changed shape.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dbus::arg::{RefArg, Variant};
use dbus::blocking::{Connection, Proxy};
use dbus::channel::{MatchingReceiver, Sender as _};
use dbus_crossroads::Crossroads;

/// Where we publish the item, and the menu beside it.
///
/// Any path works — the watcher is told which one — but the service name must be unique per item,
/// and `org.kde.StatusNotifierItem-<pid>-<n>` is the convention every host understands.
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/StatusNotifierItem/Menu";

const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const MENU_IFACE: &str = "com.canonical.dbusmenu";
const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";

/// The menu's ids. Fixed, because the menu's shape is fixed; 0 is the root, which DBusMenu
/// reserves.
const ID_SERVER: i32 = 1;
const ID_STATUS: i32 = 2;
const ID_SEP_A: i32 = 3;
const ID_TOGGLE: i32 = 4;
const ID_SEP_B: i32 = 5;
const ID_SHOW: i32 = 6;
const ID_QUIT: i32 = 7;

/// What a click on a menu line asks the app to do. `tray.rs` turns these into the same paths the
/// window uses; see its header on why the tray owns no connection logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Show,
    Quit,
}

/// A new state for the item, as the frontend painted and worded it.
pub struct Update {
    /// Straight RGBA as `trayicon.ts` produced it, row by row.
    pub rgba: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub tooltip: String,
    pub server: String,
    pub status: String,
    pub toggle: String,
    pub toggle_enabled: bool,
}

/// The item's live state, shared between the D-Bus handlers and the update channel.
struct Item {
    /// ARGB32, big-endian, which is what `IconPixmap` is defined to carry.
    argb: Vec<u8>,
    width: i32,
    height: i32,
    tooltip: String,
    server: String,
    status: String,
    toggle: String,
    toggle_enabled: bool,
    /// Bumped on every menu change, because DBusMenu clients cache a layout until the revision
    /// they hold is superseded.
    revision: u32,
}

impl Item {
    /// The five lines, in order, as `(id, label, enabled, is_separator)`.
    fn lines(&self) -> [(i32, &str, bool, bool); 7] {
        [
            (ID_SERVER, self.server.as_str(), false, false),
            (ID_STATUS, self.status.as_str(), false, false),
            (ID_SEP_A, "", true, true),
            (ID_TOGGLE, self.toggle.as_str(), self.toggle_enabled, false),
            (ID_SEP_B, "", true, true),
            (ID_SHOW, "Show Nunya", true, false),
            (ID_QUIT, "Quit Nunya", true, false),
        ]
    }

    /// One menu entry's properties, filtered to those the caller asked for.
    ///
    /// `visible` and `enabled` are sent explicitly rather than left to the spec's defaults: hosts
    /// disagree about what a missing property means, and a Connect line that a host decides to
    /// grey out on its own would contradict what the window allows.
    fn props(&self, id: i32, wanted: &[String]) -> dbus::arg::PropMap {
        let mut map = dbus::arg::PropMap::new();
        let Some((_, label, enabled, separator)) =
            self.lines().into_iter().find(|(line, ..)| *line == id)
        else {
            return map;
        };
        let mut put = |key: &str, value: Variant<Box<dyn RefArg>>| {
            if wanted.is_empty() || wanted.iter().any(|w| w == key) {
                map.insert(key.to_string(), value);
            }
        };
        if separator {
            put("type", Variant(Box::new("separator".to_string())));
        } else {
            put("label", Variant(Box::new(label.to_string())));
            put("enabled", Variant(Box::new(enabled)));
        }
        put("visible", Variant(Box::new(true)));
        map
    }
}

/// Converts the frontend's straight RGBA into `IconPixmap`'s ARGB32.
///
/// The bytes are big-endian per the spec, which is what every host assumes and what no host
/// negotiates, so getting this wrong shows as a colour-swapped icon rather than an error.
fn to_argb(rgba: &[u8]) -> Vec<u8> {
    let mut argb = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        argb.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    argb
}

/// Starts the tray on its own thread.
///
/// Returns the channel that carries state in, or an error if there is no session bus or no watcher
/// — a desktop with no tray at all, which `tray.rs` treats as "the window is the whole UI".
/// Actions come back on `actions`.
pub fn start(first: Update, actions: Sender<Action>) -> Result<Sender<Update>, String> {
    let (tx, rx) = std::sync::mpsc::channel::<Update>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    std::thread::Builder::new()
        .name("nunya-tray".into())
        .spawn(move || match serve(first, &actions, &rx, &ready_tx) {
            Ok(()) => log::info!("tray thread finished"),
            Err(e) => {
                // Only reported if it happened after the item was up; the handshake below carries
                // a startup failure to the caller instead.
                let _ = ready_tx.send(Err(e.clone()));
                log::warn!("tray stopped: {e}");
            }
        })
        .map_err(|e| e.to_string())?;

    // Blocks until the item is registered, so a desktop without a tray is reported as such before
    // the window decides that closing may hide it.
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(tx),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the tray did not come up".into()),
    }
}

/// Publishes the item and answers the bus until the update channel closes.
fn serve(
    first: Update,
    actions: &Sender<Action>,
    updates: &Receiver<Update>,
    ready: &Sender<Result<(), String>>,
) -> Result<(), String> {
    let conn = Connection::new_session().map_err(|e| e.to_string())?;

    // Unique per item, and per process: a second instance of the app must not collide with the
    // first, which would make the bus hand our name away.
    static SEQ: AtomicU32 = AtomicU32::new(1);
    let name = format!(
        "org.kde.StatusNotifierItem-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    conn.request_name(&name, false, true, false)
        .map_err(|e| e.to_string())?;

    let item = Arc::new(Mutex::new(Item {
        argb: to_argb(&first.rgba),
        width: first.width,
        height: first.height,
        tooltip: first.tooltip,
        server: first.server,
        status: first.status,
        toggle: first.toggle,
        toggle_enabled: first.toggle_enabled,
        revision: 1,
    }));

    let mut cr = Crossroads::new();
    let item_token = register_item(&mut cr);
    let menu_token = register_menu(&mut cr, actions.clone());
    cr.insert(ITEM_PATH, &[item_token], item.clone());
    cr.insert(MENU_PATH, &[menu_token], item.clone());

    conn.start_receive(
        dbus::message::MatchRule::new_method_call(),
        Box::new(move |msg, conn| {
            let _ = cr.handle_message(msg, conn);
            true
        }),
    );

    register_with_watcher(&conn, &name)?;
    let _ = ready.send(Ok(()));

    // Re-registering when the watcher reappears is what survives a `gnome-shell --replace` or an
    // extension being toggled: the shell forgets every item, and one that does not come back is a
    // tray that vanished until the app restarts.
    watch_for_watcher(&conn, &name);

    loop {
        conn.process(Duration::from_millis(200))
            .map_err(|e| e.to_string())?;

        // Coalesced: the frontend can report several times between two turns of this loop, and
        // only the last one describes the tray now.
        let mut latest = None;
        loop {
            match updates.try_recv() {
                Ok(update) => latest = Some(update),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                // The app is going away.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if let Some(update) = latest {
            apply(&conn, &item, update);
        }
    }
}

/// Writes a new state in and tells the shell what changed.
///
/// The two signals are separate because hosts act on them separately: `NewIcon` re-reads the
/// pixmap, `LayoutUpdated` re-reads the menu. Emitting only one leaves the other stale.
fn apply(conn: &Connection, item: &Arc<Mutex<Item>>, update: Update) {
    let revision = {
        let mut item = match item.lock() {
            Ok(item) => item,
            // A handler panicked; the item's words would be a guess from here on.
            Err(_) => return,
        };
        item.argb = to_argb(&update.rgba);
        item.width = update.width;
        item.height = update.height;
        item.tooltip = update.tooltip;
        item.server = update.server;
        item.status = update.status;
        item.toggle = update.toggle;
        item.toggle_enabled = update.toggle_enabled;
        item.revision = item.revision.wrapping_add(1);
        item.revision
    };

    let icon = dbus::Message::signal(&ITEM_PATH.into(), &ITEM_IFACE.into(), &"NewIcon".into());
    let tooltip = dbus::Message::signal(&ITEM_PATH.into(), &ITEM_IFACE.into(), &"NewToolTip".into());
    // -1 is DBusMenu's "the whole menu", which is what a relabelled line amounts to here.
    let layout = dbus::Message::signal(
        &MENU_PATH.into(),
        &MENU_IFACE.into(),
        &"LayoutUpdated".into(),
    )
    .append2(revision, -1i32);
    for signal in [icon, tooltip, layout] {
        let _ = conn.send(signal);
    }
}

/// Asks the shell to show the item.
fn register_with_watcher(conn: &Connection, name: &str) -> Result<(), String> {
    let proxy = Proxy::new(
        WATCHER_NAME,
        WATCHER_PATH,
        Duration::from_secs(5),
        conn,
    );
    proxy
        .method_call::<(), _, _, _>(
            WATCHER_NAME,
            "RegisterStatusNotifierItem",
            (name.to_string(),),
        )
        .map_err(|e| format!("no tray on this desktop: {e}"))
}

/// `org.freedesktop.DBus.NameOwnerChanged`.
///
/// Declared here because dbus 0.9 keeps its own generated copy `pub(crate)`, so it is not something
/// a dependent crate can name.
struct NameOwnerChanged {
    name: String,
    _old_owner: String,
    new_owner: String,
}

impl dbus::arg::ReadAll for NameOwnerChanged {
    fn read(i: &mut dbus::arg::Iter) -> Result<Self, dbus::arg::TypeMismatchError> {
        Ok(NameOwnerChanged {
            name: i.read()?,
            _old_owner: i.read()?,
            new_owner: i.read()?,
        })
    }
}

impl dbus::message::SignalArgs for NameOwnerChanged {
    const NAME: &'static str = "NameOwnerChanged";
    const INTERFACE: &'static str = "org.freedesktop.DBus";
}

/// Re-registers whenever the watcher comes back, so the icon survives a shell restart.
fn watch_for_watcher(conn: &Connection, name: &str) {
    use dbus::message::SignalArgs;

    let mut rule = NameOwnerChanged::match_rule(None, None).static_clone();
    rule.msg_type = Some(dbus::MessageType::Signal);
    let name = name.to_string();
    let _ = conn.add_match(rule, move |changed: NameOwnerChanged, conn, _| {
        if changed.name == WATCHER_NAME && !changed.new_owner.is_empty() {
            log::info!("tray watcher reappeared; registering again");
            if let Err(e) = register_with_watcher(conn, &name) {
                log::warn!("could not re-register the tray: {e}");
            }
        }
        true
    });
}

/// The item interface — with no `Activate` method, which is the point; see the module header.
fn register_item(cr: &mut Crossroads) -> dbus_crossroads::IfaceToken<Arc<Mutex<Item>>> {
    cr.register(ITEM_IFACE, |b| {
        b.property("Category")
            .get(|_, _| Ok("SystemServices".to_string()));
        b.property("Id").get(|_, _| Ok("nunya".to_string()));
        b.property("Title").get(|_, _| Ok("Nunya".to_string()));
        b.property("Status").get(|_, _| Ok("Active".to_string()));
        // True because this item's whole interaction is its menu. Hosts that honour it skip
        // activation entirely and open the menu on a left click.
        b.property("ItemIsMenu").get(|_, _| Ok(true));
        b.property("Menu")
            .get(|_, _| Ok(dbus::Path::from(MENU_PATH).into_static()));
        // Empty, so hosts fall through to the pixmap rather than hunting the icon theme for a
        // name that is not in it.
        b.property("IconName").get(|_, _| Ok(String::new()));
        b.property("IconPixmap").get(|_, item: &mut Arc<Mutex<Item>>| {
            let item = item.lock().map_err(|_| {
                dbus::MethodErr::failed(&"the tray icon is unreadable")
            })?;
            Ok(vec![(item.width, item.height, item.argb.clone())])
        });
        b.property("ToolTip").get(|_, item: &mut Arc<Mutex<Item>>| {
            let item = item.lock().map_err(|_| {
                dbus::MethodErr::failed(&"the tray tooltip is unreadable")
            })?;
            // (icon name, icon pixmaps, title, body): the title carries it, and the icon is
            // already in the bar beside it.
            Ok((
                String::new(),
                Vec::<(i32, i32, Vec<u8>)>::new(),
                item.tooltip.clone(),
                String::new(),
            ))
        });
        b.signal::<(), _>("NewIcon", ());
        b.signal::<(), _>("NewToolTip", ());
        b.signal::<(String,), _>("NewStatus", ("status",));
        b.signal::<(String,), _>("NewTitle", ("title",));
        // Scroll is answered because hosts call it unprompted and an unknown-method reply is
        // logged as an error by some of them; there is nothing sensible to scroll here.
        b.method("Scroll", ("delta", "orientation"), (), |_, _, _: (i32, String)| Ok(()));
        // A middle click. Mapped to showing the window, which is the one harmless thing it could
        // mean, and never to connecting: a gesture nobody aimed must not move the tunnel.
        b.method(
            "SecondaryActivate",
            ("x", "y"),
            (),
            |_, _, _: (i32, i32)| Ok(()),
        );
    })
}

/// The DBusMenu interface: the seven fixed lines, and the click that comes back.
fn register_menu(
    cr: &mut Crossroads,
    actions: Sender<Action>,
) -> dbus_crossroads::IfaceToken<Arc<Mutex<Item>>> {
    cr.register(MENU_IFACE, |b| {
        b.property("Version").get(|_, _| Ok(3u32));
        b.property("Status").get(|_, _| Ok("normal".to_string()));
        b.property("TextDirection")
            .get(|_, _| Ok("ltr".to_string()));
        b.property("IconThemePath")
            .get(|_, _| Ok(Vec::<String>::new()));

        /// One node: its id, its properties, and its children as variants — DBusMenu's
        /// `(ia{sv}av)`, which is recursive and so cannot be typed any more precisely.
        type Layout = (i32, dbus::arg::PropMap, Vec<Variant<Box<dyn RefArg>>>);
        /// One entry of `EventGroup`: id, event name, the event's data, and its timestamp.
        type Event = (i32, String, Variant<Box<dyn RefArg>>, u32);

        b.method(
            "GetLayout",
            ("parentId", "recursionDepth", "propertyNames"),
            ("revision", "layout"),
            |_, item: &mut Arc<Mutex<Item>>, (parent, _depth, wanted): (i32, i32, Vec<String>)| {
                let item = item
                    .lock()
                    .map_err(|_| dbus::MethodErr::failed(&"the tray menu is unreadable"))?;
                // Only the root has children; a leaf asked for itself gets itself.
                if parent != 0 {
                    let props = item.props(parent, &wanted);
                    let layout: Layout = (parent, props, vec![]);
                    return Ok((item.revision, layout));
                }
                let children = item
                    .lines()
                    .into_iter()
                    .map(|(id, ..)| {
                        let props = item.props(id, &wanted);
                        Variant(Box::new((id, props, Vec::<Variant<Box<dyn RefArg>>>::new()))
                            as Box<dyn RefArg>)
                    })
                    .collect();
                let mut root = dbus::arg::PropMap::new();
                // Without this a host has no reason to believe the root opens into anything, and
                // some draw nothing at all.
                root.insert(
                    "children-display".to_string(),
                    Variant(Box::new("submenu".to_string())),
                );
                let layout: Layout = (0, root, children);
                Ok((item.revision, layout))
            },
        );

        b.method(
            "GetGroupProperties",
            ("ids", "propertyNames"),
            ("properties",),
            |_, item: &mut Arc<Mutex<Item>>, (ids, wanted): (Vec<i32>, Vec<String>)| {
                let item = item
                    .lock()
                    .map_err(|_| dbus::MethodErr::failed(&"the tray menu is unreadable"))?;
                // An empty list means every item, which is how hosts refresh a whole menu.
                let ids = if ids.is_empty() {
                    item.lines().into_iter().map(|(id, ..)| id).collect()
                } else {
                    ids
                };
                let out: Vec<(i32, dbus::arg::PropMap)> = ids
                    .into_iter()
                    .map(|id| (id, item.props(id, &wanted)))
                    .collect();
                Ok((out,))
            },
        );

        b.method(
            "GetProperty",
            ("id", "name"),
            ("value",),
            |_, item: &mut Arc<Mutex<Item>>, (id, name): (i32, String)| {
                let item = item
                    .lock()
                    .map_err(|_| dbus::MethodErr::failed(&"the tray menu is unreadable"))?;
                let wanted = [name.clone()];
                item.props(id, &wanted)
                    .remove(&name)
                    .map(|value| (value,))
                    .ok_or_else(|| dbus::MethodErr::failed(&"no such menu property"))
            },
        );

        let clicked = actions.clone();
        b.method(
            "Event",
            ("id", "eventId", "data", "timestamp"),
            (),
            move |_, item: &mut Arc<Mutex<Item>>, (id, event, _data, _ts): (
                i32,
                String,
                Variant<Box<dyn RefArg>>,
                u32,
            )| {
                if event != "clicked" {
                    // "hovered", "opened" and "closed" all arrive here and mean nothing to us.
                    return Ok(());
                }
                // A disabled line is not a command. Hosts are expected to refuse the click
                // themselves, but the toggle's enabled state is the one thing here that says
                // whether the window would accept a connect, so it is checked rather than trusted.
                if id == ID_TOGGLE {
                    let enabled = item
                        .lock()
                        .map(|item| item.toggle_enabled)
                        .unwrap_or(false);
                    if !enabled {
                        return Ok(());
                    }
                }
                let action = match id {
                    ID_TOGGLE => Some(Action::Toggle),
                    ID_SHOW => Some(Action::Show),
                    ID_QUIT => Some(Action::Quit),
                    _ => None,
                };
                if let Some(action) = action {
                    let _ = clicked.send(action);
                }
                Ok(())
            },
        );

        // Hosts call these before drawing. Nothing is built lazily here, so the answer is always
        // "no update needed" — but they must exist, or a host that waits for the reply draws an
        // empty menu.
        b.method(
            "AboutToShow",
            ("id",),
            ("needUpdate",),
            |_, _, _: (i32,)| Ok((false,)),
        );
        b.method(
            "AboutToShowGroup",
            ("ids",),
            ("updatesNeeded", "idErrors"),
            |_, _, _: (Vec<i32>,)| Ok((Vec::<i32>::new(), Vec::<i32>::new())),
        );
        b.method(
            "EventGroup",
            ("events",),
            ("idErrors",),
            |_, _, _: (Vec<Event>,)| Ok((Vec::<i32>::new(),)),
        );

        b.signal::<(u32, i32), _>("LayoutUpdated", ("revision", "parent"));
        b.signal::<(Vec<(i32, dbus::arg::PropMap)>, Vec<(i32, Vec<String>)>), _>(
            "ItemsPropertiesUpdated",
            ("updatedProps", "removedProps"),
        );
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> Item {
        Item {
            argb: vec![],
            width: 44,
            height: 44,
            tooltip: "Nunya — Disconnected".into(),
            server: "Server: DE-1 Frankfurt".into(),
            status: "Status: Disconnected".into(),
            toggle: "Connect".into(),
            toggle_enabled: true,
            revision: 1,
        }
    }

    #[test]
    fn rgba_becomes_argb_with_the_alpha_moved_to_the_front() {
        // One opaque red pixel: R,G,B,A in becomes A,R,G,B out.
        assert_eq!(to_argb(&[0x10, 0x20, 0x30, 0xff]), vec![0xff, 0x10, 0x20, 0x30]);
    }

    #[test]
    fn the_menu_is_the_seven_lines_in_the_order_the_window_shows_them() {
        let item = item();
        let ids: Vec<i32> = item.lines().into_iter().map(|(id, ..)| id).collect();
        assert_eq!(ids, vec![
            ID_SERVER, ID_STATUS, ID_SEP_A, ID_TOGGLE, ID_SEP_B, ID_SHOW, ID_QUIT
        ]);
    }

    #[test]
    fn a_separator_is_typed_as_one_and_carries_no_label() {
        let item = item();
        let props = item.props(ID_SEP_A, &[]);
        assert_eq!(
            props.get("type").map(|v| format!("{:?}", v.0)),
            Some(format!("{:?}", "separator".to_string()))
        );
        assert!(!props.contains_key("label"));
    }

    #[test]
    fn every_line_states_visible_and_enabled_rather_than_leaving_them_to_a_default() {
        let item = item();
        for (id, _, _, separator) in item.lines() {
            let props = item.props(id, &[]);
            assert!(props.contains_key("visible"), "id {id} has no visible");
            if !separator {
                assert!(props.contains_key("enabled"), "id {id} has no enabled");
            }
        }
    }

    #[test]
    fn the_labels_are_the_words_the_frontend_sent() {
        let item = item();
        let label = |id| {
            item.props(id, &["label".to_string()])
                .get("label")
                .map(|v| format!("{:?}", v.0))
        };
        assert_eq!(label(ID_SERVER), Some(format!("{:?}", "Server: DE-1 Frankfurt".to_string())));
        assert_eq!(label(ID_TOGGLE), Some(format!("{:?}", "Connect".to_string())));
    }

    #[test]
    fn asking_for_one_property_returns_only_that_one() {
        let item = item();
        let props = item.props(ID_TOGGLE, &["label".to_string()]);
        assert!(props.contains_key("label"));
        assert!(!props.contains_key("enabled"));
        assert!(!props.contains_key("visible"));
    }

    #[test]
    fn an_unknown_id_has_no_properties_rather_than_a_panic() {
        assert!(item().props(999, &[]).is_empty());
    }
}
