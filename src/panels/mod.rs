/*
 * panel manager: tracks which panels are open and enforces the relationships between them
 *
 * panels are layer windows that always exist and are shown or hidden through `.visible`,
 * since amane can't open or close layer windows at runtime
 *
 * from a keybind: `amane ipc call panel toggle launcher`
 */

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amane::{
    Animation, Key, Keyboard, Layer, LayerWindow, Monitor, Rectangle, Service, Size::Parent,
    TextInput, Workspaces,
};

use crate::animations::{self, Animations};
use crate::theme::{Theme, theme};

pub mod audio;
pub mod backdrop;
pub mod bluetooth;
pub mod calendar;
pub mod clipboard;
mod hints;
pub mod keybinds;
pub mod launcher;
pub mod layout;
mod list;
mod menu;
pub mod network;
pub mod notifications;
mod picker;
pub mod screenshot;
pub mod session;
pub mod system;
pub mod ui;
pub mod wallpaper;

pub const LAUNCHER: &str = "launcher";
pub const CLIPBOARD: &str = "clipboard";
pub const NOTIFICATIONS: &str = "notifications";
pub const SCREENSHOT: &str = "screenshot";
pub const SESSION: &str = "session";
pub const NETWORK: &str = "network";
pub const BLUETOOTH: &str = "bluetooth";
pub const AUDIO: &str = "audio";
pub const LAYOUT: &str = "layout";
pub const SYSTEM: &str = "system";
pub const CALENDAR: &str = "calendar";
pub const WALLPAPER: &str = "wallpaper";
pub const KEYBINDS: &str = "keybinds";

pub struct PanelDef {
    pub name: &'static str,

    // what it's for, shown next to its name in the launcher's : list
    pub description: &'static str,

    // opening a panel closes every open panel that shares at least one group with it
    pub groups: &'static [&'static str],

    /*
     * runs whenever the panel closes, however that happens, to clear its search and
     * selection; None when there is nothing to clear. it runs on the thread that
     * closed the panel, which is the draw thread for keys, clicks and ipc calls;
     * TextInput's text is kept per thread, so a hook run from another thread
     * (the launcher's ! calls run their handler on one) clears nothing there.
     * close panels with a TextInput only from the draw thread
     */
    pub on_close: Option<fn()>,

    // sits in the top-right corner, where notification popups show, which hide while it's open
    pub corner: bool,
}

/*
 * the group every panel is in, so only one is ever open: when a panel that has
 * the keyboard closes, mango gives the keyboard to a window, never to another
 * panel, so escape could no longer reach a panel left open underneath
 *
 * except the screenshot menu, which opens on top so open panels can be captured
 * (its window is made last in main.rs, so it draws over them);
 * the panel underneath loses the keyboard once the menu (or slurp) closes, and
 * gets it back when clicked
 */
const PANELS: &str = "panels";

// every panel and its relationships, the single place to configure them
static REGISTRY: &[PanelDef] = &[
    PanelDef {
        name: LAUNCHER,
        description: "Search and start apps",
        groups: &[PANELS],
        on_close: Some(launcher::reset),
        corner: false,
    },
    PanelDef {
        name: CLIPBOARD,
        description: "Clipboard history",
        groups: &[PANELS],
        on_close: Some(clipboard::reset),
        corner: false,
    },
    PanelDef {
        name: SCREENSHOT,
        description: "Screenshots and recordings",
        groups: &[],
        on_close: Some(screenshot::reset),
        corner: false,
    },
    PanelDef {
        name: SESSION,
        description: "Lock, suspend, log out, reboot, shut down",
        groups: &[PANELS],
        on_close: Some(session::reset),
        corner: false,
    },
    PanelDef {
        name: LAYOUT,
        description: "Window layouts",
        groups: &[PANELS],
        on_close: Some(layout::reset),
        corner: false,
    },
    PanelDef {
        name: NOTIFICATIONS,
        description: "Notification center",
        groups: &[PANELS],
        on_close: Some(notifications::reset),
        corner: true,
    },
    PanelDef {
        name: NETWORK,
        description: "Wi-Fi and VPN",
        groups: &[PANELS],
        on_close: Some(network::reset),
        corner: true,
    },
    PanelDef {
        name: BLUETOOTH,
        description: "Bluetooth devices",
        groups: &[PANELS],
        on_close: Some(bluetooth::reset),
        corner: true,
    },
    PanelDef {
        name: AUDIO,
        description: "Volume, outputs and inputs",
        groups: &[PANELS],
        on_close: Some(audio::reset),
        corner: true,
    },
    PanelDef {
        name: SYSTEM,
        description: "System info and settings",
        groups: &[PANELS],
        on_close: None,
        corner: true,
    },
    PanelDef {
        name: CALENDAR,
        description: "Calendar and weather",
        groups: &[PANELS],
        on_close: Some(calendar::reset),
        corner: false,
    },
    PanelDef {
        name: WALLPAPER,
        description: "Choose from your wallpapers",
        groups: &[PANELS],
        on_close: Some(wallpaper::reset),
        corner: false,
    },
    PanelDef {
        name: KEYBINDS,
        description: "Every keybind",
        groups: &[PANELS],
        on_close: Some(keybinds::reset),
        corner: false,
    },
];

/*
 * the panels the launcher's : list can open: all but the launcher itself and
 * the screenshot menu, which shares no group with the launcher, so it would show
 * while the launcher hides (the layer surface crash), and has nothing to capture
 */
pub fn launchable() -> impl Iterator<Item = &'static PanelDef> {
    REGISTRY
        .iter()
        .filter(|panel| panel.name != LAUNCHER && panel.name != SCREENSHOT)
}

fn find(name: &str) -> Option<&'static PanelDef> {
    REGISTRY.iter().find(|panel| panel.name == name)
}

fn conflicts(a: &PanelDef, b: &PanelDef) -> bool {
    a.name != b.name && a.groups.iter().any(|group| b.groups.contains(group))
}

struct OpenPanel {
    def: &'static PanelDef,

    // the monitor that was focused when the panel opened; None shows it everywhere
    output: Option<String>,
}

pub struct Panels {
    // oldest first
    open: Vec<OpenPanel>,

    /*
     * closed but still mapped: fading out, or waiting their turn to hide
     * (see hide_time); each hides at its own moment
     */
    closing: Vec<Closing>,

    // bumped on every change, so a delayed open can tell it was overtaken
    generation: u64,

    // tells the closing panels apart, so each one's hide finds only itself
    closing_serial: u64,

    /*
     * the last moment a panel showed or is due to hide: the next show or hide
     * waits until SWAP_DELAY after it
     */
    last_change: Option<Instant>,
}

// a closed panel still mapped until hides_at
struct Closing {
    panel: OpenPanel,
    serial: u64,
    hides_at: Instant,
}

/*
 * the least time between two panels showing or hiding: showing or hiding a layer
 * surface makes mango reconfigure every surface on the monitor, and a configure
 * that crosses another panel's unmap gets acked after wlroots has dropped it,
 * which kills the shell with a protocol error. so a panel replacing another waits
 * this long after the old one hides, and when several close at once (close-all
 * with the screenshot menu over a panel) they hide this far apart
 */
const SWAP_DELAY: Duration = Duration::from_millis(100);

/*
 * how long a panel fades in as it opens and out as it closes; a closing panel
 * hides once its fade is done, and any panel opening meanwhile waits for that
 * plus SWAP_DELAY, so a show never crosses a hide (the crash above)
 */
const FADE: Duration = Duration::from_millis(100);

/*
 * how long until another surface may show, after closing a panel: until the
 * last closing panel has hidden, then SWAP_DELAY. for work that may show one
 * itself, like the launcher's ! calls (`!record menu` opens the screenshot
 * menu, which replaces nothing). takes the read lock, so not inside a view
 */
pub fn until_closed() -> Duration {
    Panels::read().wait_for_quiet(Instant::now())
}

/*
 * when a panel closed at `now` may hide: after its fade when animations are on,
 * and never sooner than SWAP_DELAY after the last show or hide, so two
 * surfaces never change at the same moment
 */
fn hide_time(now: Instant, fade: bool, last_change: Option<Instant>) -> Instant {
    let earliest = if fade { now + FADE } else { now };

    match last_change {
        Some(last) => earliest.max(last + SWAP_DELAY),
        None => earliest,
    }
}

impl Service for Panels {
    fn new() -> Self {
        Self {
            open: Vec::new(),
            closing: Vec::new(),
            generation: 0,
            closing_serial: 0,
            last_change: None,
        }
    }

    // only changes through input and ipc
    fn listen() {}
}

// reading, safe inside views
impl Panels {
    pub fn is_open(&self, name: &str) -> bool {
        self.open.iter().any(|panel| panel.def.name == name)
    }

    pub fn any_open(&self) -> bool {
        !self.open.is_empty()
    }

    // whether a panel in the top-right corner is open
    pub fn corner_open(&self) -> bool {
        self.open.iter().any(|panel| panel.def.corner)
    }

    pub fn open_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.open.iter().map(|panel| panel.def.name)
    }

    pub fn shown_on(&self, name: &str, monitor: &Monitor) -> bool {
        self.open.iter().any(|panel| panel.is_on(name, monitor))
    }

    // closed, but still mapped on this monitor (fading out, or waiting to hide)
    pub fn closing_on(&self, name: &str, monitor: &Monitor) -> bool {
        self.closing
            .iter()
            .any(|closing| closing.panel.is_on(name, monitor))
    }

    // a panel other than this one is still to hide, so showing this one must wait
    fn others_closing(&self, name: &str) -> bool {
        self.closing
            .iter()
            .any(|closing| closing.panel.def.name != name)
    }

    fn is_closing(&self, name: &str) -> bool {
        self.closing
            .iter()
            .any(|closing| closing.panel.def.name == name)
    }

    /*
     * the first moment another surface may show or hide, SWAP_DELAY after the
     * last show or hide (done or due); None before any panel showed
     */
    fn quiet_at(&self) -> Option<Instant> {
        let last_hide = self.closing.iter().map(|closing| closing.hides_at).max();

        let last = match (last_hide, self.last_change) {
            (Some(hide), Some(change)) => hide.max(change),
            (hide, change) => hide.or(change)?,
        };

        Some(last + SWAP_DELAY)
    }

    // how long from now until quiet_at; zero when that has passed
    fn wait_for_quiet(&self, now: Instant) -> Duration {
        self.quiet_at()
            .map_or(Duration::ZERO, |quiet| quiet.saturating_duration_since(now))
    }
}

impl OpenPanel {
    fn is_on(&self, name: &str, monitor: &Monitor) -> bool {
        self.def.name == name
            && self
                .output
                .as_deref()
                .is_none_or(|output| output == monitor.name)
    }
}

/*
 * writing; these take the write lock, so never call them from inside a view,
 * only from input handlers and ipc
 */
impl Panels {
    pub fn open(name: &str) -> Result<(), String> {
        let def = find(name).ok_or_else(|| unknown(name))?;
        let output = focused_output();
        let fade = Animations::read().enabled();

        let mut panels = Panels::write();

        let replaced = panels.remove(|other| conflicts(def, other), fade);

        panels.generation += 1;

        /*
         * reopening one that's still mapped (fading out, or waiting its turn to
         * hide) keeps it mapped, so it can show at once
         */
        let reopening =
            replaced.is_empty() && panels.is_closing(def.name) && !panels.others_closing(def.name);

        /*
         * anything replaced hides now or soon (see remove), which makes this
         * wait; so does any other show or hide less than SWAP_DELAY ago
         */
        let delay = panels.wait_for_quiet(Instant::now());

        if reopening || delay.is_zero() {
            panels.show(def, output);

            drop(panels);

            run_close_hooks(&replaced);

            return Ok(());
        }

        // the replaced panels hide now (or after fading), this one shows once they are gone
        let generation = panels.generation;

        drop(panels);

        run_close_hooks(&replaced);

        thread::spawn(move || {
            thread::sleep(delay);

            let mut panels = Panels::write();

            // anything that happened in between wins
            if panels.generation == generation {
                panels.show(def, output);
            }
        });

        Ok(())
    }

    pub fn close(name: &str) -> Result<(), String> {
        find(name).ok_or_else(|| unknown(name))?;

        let fade = Animations::read().enabled();

        let closed = Panels::write().remove(|other| other.name == name, fade);

        run_close_hooks(&closed);

        Ok(())
    }

    // returns whether the panel is open afterwards
    pub fn toggle(name: &str) -> Result<bool, String> {
        if Panels::read().is_open(name) {
            Panels::close(name)?;

            Ok(false)
        } else {
            Panels::open(name)?;

            Ok(true)
        }
    }

    pub fn close_all() {
        let fade = Animations::read().enabled();

        let closed = Panels::write().remove(|_| true, fade);

        run_close_hooks(&closed);
    }

    /*
     * takes out the open panels that match, and bumps the generation. each
     * hides at its own hide_time: one closed with nothing else changing just
     * before hides with the next frame, as do all of them without animations;
     * the rest stay mapped (fading out, or invisible while they wait) until a
     * thread hides them, one at a time, SWAP_DELAY apart. newest first, so the
     * screenshot menu goes before the panel under it
     */
    fn remove(
        &mut self,
        matches: impl Fn(&PanelDef) -> bool,
        fade: bool,
    ) -> Vec<&'static PanelDef> {
        let (removed, kept): (Vec<OpenPanel>, Vec<OpenPanel>) =
            self.open.drain(..).partition(|panel| matches(panel.def));

        self.open = kept;
        self.generation += 1;

        let names = removed.iter().map(|panel| panel.def).collect();

        let now = Instant::now();

        for panel in removed.into_iter().rev() {
            let hides_at = hide_time(now, fade, self.last_change);

            self.last_change = Some(hides_at);

            // nothing changed just before and nothing to fade: gone with the next frame
            if hides_at <= now {
                continue;
            }

            self.closing_serial += 1;

            let serial = self.closing_serial;

            self.closing.push(Closing {
                panel,
                serial,
                hides_at,
            });

            thread::spawn(move || {
                thread::sleep(hides_at.saturating_duration_since(Instant::now()));

                Panels::write()
                    .closing
                    .retain(|closing| closing.serial != serial);
            });
        }

        names
    }

    // maps the panel (or keeps it mapped, when it was still closing); counts as a change
    fn show(&mut self, def: &'static PanelDef, output: Option<String>) {
        self.last_change = Some(Instant::now());

        if let Some(panel) = self
            .open
            .iter_mut()
            .find(|panel| panel.def.name == def.name)
        {
            panel.output = output;
        } else {
            self.open.push(OpenPanel { def, output });
        }
    }
}

// called without the Panels lock held, so hooks may read or write any service
fn run_close_hooks(closed: &[&'static PanelDef]) {
    if !closed.is_empty() {
        hints::hide();
    }

    for def in closed {
        if let Some(on_close) = def.on_close {
            on_close();
        }
    }
}

fn unknown(name: &str) -> String {
    let known: Vec<&str> = REGISTRY.iter().map(|panel| panel.name).collect();

    format!("unknown panel '{name}', known panels: {}", known.join(", "))
}

// whether this monitor has focus; true everywhere when the compositor doesn't say
pub fn is_focused(monitor: &Monitor) -> bool {
    focused_output().is_none_or(|output| output == monitor.name)
}

pub fn focused_output() -> Option<String> {
    Workspaces::read()
        .list()
        .iter()
        .find(|workspace| workspace.focused())
        .and_then(|workspace| workspace.output())
        .map(String::from)
}

const USAGE: &str = "usage: panel open|close|toggle <name> | panel close-all | panel list";

// `amane ipc call panel <command> [name]`
pub fn ipc(arguments: &[String]) -> String {
    let command = arguments.first().map(String::as_str);
    let name = arguments.get(1).map(String::as_str);

    let result = match (command, name) {
        (Some("open"), Some(name)) => Panels::open(name).map(|_| "opened"),
        (Some("close"), Some(name)) => Panels::close(name).map(|_| "closed"),
        (Some("toggle"), Some(name)) => {
            Panels::toggle(name).map(|open| if open { "opened" } else { "closed" })
        }
        (Some("close-all"), None) => {
            Panels::close_all();

            Ok("closed")
        }
        (Some("list"), None) => {
            let names: Vec<&str> = Panels::read().open_names().collect();

            return names.join("\n");
        }
        _ => Err(String::from(USAGE)),
    };

    match result {
        Ok(reply) => String::from(reply),
        Err(error) => error,
    }
}

// the letter a key typed, lowercased, so a held Shift doesn't change what it does; None for other keys
pub fn letter(key: &Key) -> Option<char> {
    match key {
        Key::Character(letter) => Some(letter.to_ascii_lowercase()),
        _ => None,
    }
}

/*
 * a panel's text input that takes typing as soon as the panel opens, except
 * while the screenshot menu is open on top: amane has one focused input for
 * the whole shell, and it takes every typed character whichever window has
 * the keyboard, so the menu's number keys would land in the search below.
 * a focused input claims the focus again on each redraw, so it has to stop
 * asking; mango's keyboard leave clears what it already had
 */
pub fn focused(input: TextInput) -> TextInput {
    if Panels::read().is_open(SCREENSHOT) {
        input
    } else {
        input.focused()
    }
}

/*
 * for Escape: whether the panel closed something inside it instead, like the
 * network panel's password box (a text input lets go of Escape and passes it
 * on, so it would close the whole panel and lose your place in the list)
 */
fn backs_out(name: &str) -> bool {
    match name {
        NETWORK => network::back_out(),
        _ => false,
    }
}

/*
 * each panel's opacity by panel and monitor: towards 1 while open, towards 0
 * while closing (see Panels::closing). the view is built anew on every
 * redraw, so the animations are kept here
 */
static FADES: Mutex<Option<HashMap<(&'static str, String), Animation<f32>>>> = Mutex::new(None);

// a hidden panel goes back to 0, so it fades in again the next time it opens
fn opacity(name: &'static str, monitor: &str, open: bool, visible: bool) -> f32 {
    let mut fades = FADES.lock().unwrap_or_else(PoisonError::into_inner);
    let fades = fades.get_or_insert_with(HashMap::new);

    let animation = fades
        .entry((name, monitor.to_string()))
        .or_insert_with(|| Animation::new(0.0).duration(FADE));

    if !visible {
        *animation = Animation::new(0.0).duration(FADE);

        return 0.0;
    }

    let target = if open { 1.0 } else { 0.0 };

    animations::follow(animation, target, Animations::read().enabled())
}

/*
 * every panel's window: shown on the monitor it was opened on, above the
 * backdrop, taking the keyboard and closing on escape; every other key that
 * reaches the window goes to the panel's own handler, except ? which shows the
 * key hints (see hints.rs)
 *
 * content is only built while the panel shows. a hidden window still runs its
 * view whenever a service it read changes, so a panel that read the network or
 * the volume while hidden would run again on every signal-strength or volume
 * change; built lazily, a hidden panel reads nothing but Panels
 *
 * on-demand rather than exclusive: amane keeps a hidden window's last keyboard mode,
 * and mango hands exclusive focus to any exclusive layer surface, even unmapped ones,
 * so a hidden exclusive panel would keep stealing focus from your windows.
 * mango still focuses an on-demand surface as soon as it shows, and the backdrop
 * covers everything else, so the panel keeps the keyboard while it's open
 */
pub fn panel(
    monitor: &Monitor,
    name: &'static str,
    base: LayerWindow,
    on_key: fn(Key),
    content: impl FnOnce(&Theme) -> Rectangle,
) -> LayerWindow {
    let (open, visible) = {
        let panels = Panels::read();
        let open = panels.shown_on(name, monitor);

        (open, open || panels.closing_on(name, monitor))
    };

    let opacity = opacity(name, &monitor.name, open, visible);

    // a window needs a child even while hidden, though nothing draws it
    let child = if visible {
        Rectangle::new()
            .width(Parent)
            .height(Parent)
            .opacity(opacity)
            .child(content(&theme()))
    } else {
        Rectangle::new().width(1.0).height(1.0)
    };

    base.layer(Layer::Overlay)
        .visible(visible)
        .keyboard(Keyboard::OnDemand)
        .on_key(move |key| {
            // a panel fading out takes no more keys
            if !Panels::read().is_open(name) {
                return;
            }

            match key {
                // something open inside the panel closes first, the panel on the next press
                Key::Escape if backs_out(name) => {}

                Key::Escape => {
                    let _ = Panels::close(name);
                }

                // every panel shows its buttons' keys for a moment
                Key::Character('?') => hints::show(),

                _ => on_key(key),
            }
        })
        .child(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(name: &'static str) -> OpenPanel {
        OpenPanel {
            def: find(name).expect("a panel"),
            output: None,
        }
    }

    #[test]
    fn a_panel_closed_alone_hides_at_once() {
        let now = Instant::now();
        let long_ago = now.checked_sub(Duration::from_secs(5)).expect("a time");

        assert_eq!(hide_time(now, false, None), now);
        assert_eq!(hide_time(now, false, Some(long_ago)), now);
        assert_eq!(hide_time(now, true, Some(long_ago)), now + FADE);
    }

    #[test]
    fn panels_closed_together_hide_one_after_another() {
        let now = Instant::now();

        let first = hide_time(now, false, None);
        let second = hide_time(now, false, Some(first));
        let third = hide_time(now, false, Some(second));

        assert_eq!(second - first, SWAP_DELAY);
        assert_eq!(third - second, SWAP_DELAY);

        let faded = hide_time(now, true, None);

        assert_eq!(hide_time(now, true, Some(faded)), now + FADE + SWAP_DELAY);
    }

    #[test]
    fn a_change_just_before_holds_the_hide_back() {
        let now = Instant::now();
        let shown = now.checked_sub(Duration::from_millis(30)).expect("a time");

        assert_eq!(hide_time(now, false, Some(shown)), shown + SWAP_DELAY);
    }

    #[test]
    fn close_all_never_hides_two_at_once() {
        let mut panels = Panels::new();

        panels.open = vec![open(NETWORK), open(SCREENSHOT)];

        let closed = panels.remove(|_| true, false);

        assert_eq!(closed.len(), 2);

        // the menu on top hides with the next frame, the panel under it waits its turn
        assert_eq!(panels.closing.len(), 1);
        assert_eq!(panels.closing[0].panel.def.name, NETWORK);

        let now = Instant::now();
        let hides_in = panels.closing[0].hides_at.saturating_duration_since(now);

        assert!(hides_in > Duration::ZERO && hides_in <= SWAP_DELAY);

        // and whatever shows next waits until SWAP_DELAY after that
        assert_eq!(
            panels.quiet_at(),
            Some(panels.closing[0].hides_at + SWAP_DELAY)
        );
        assert!(panels.wait_for_quiet(now) > SWAP_DELAY);
    }

    #[test]
    fn nothing_waits_before_anything_showed() {
        let panels = Panels::new();

        assert_eq!(panels.quiet_at(), None);
        assert_eq!(panels.wait_for_quiet(Instant::now()), Duration::ZERO);
    }
}
