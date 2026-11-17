/*
 * eye breaks, the 20-20-20 habit: every 20 minutes a popup asks you to look at
 * something far away for 20 seconds, counts those down, goes away and the 20
 * minutes start again. switched in the system panel (kept in a state file,
 * off unless switched on), where the time to the next break shows too
 *
 * the cycle itself is a timer::Cycle, ticked once a second on this service's
 * thread. locking restarts it: amane doesn't say whether the session is
 * locked, but the lock screen only exists while it is and redraws at least
 * every minute (its clock), and each draw restarts the cycle, so the next
 * break comes 20 minutes after unlocking rather than right away. a suspend
 * locks first, and the clock the cycle runs on stands still while asleep
 *
 * the popup window is always mapped, never changes size and lets every click
 * through; it only draws while a break is on, like the volume popup (osd.rs)
 */

use std::fs;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amane::{
    Align::Center, Color, Full, Keyboard, Layer, LayerWindow, Margin, Monitor, Rectangle, Service,
    Size::Parent, Vertical,
};

use crate::panels;
use crate::state;
use crate::theme::theme;
use crate::timer::{self, Cycle};

// the file in the state folder the setting is kept in
const STATE: &str = "eye-breaks";

const ON: &str = "on";
const OFF: &str = "off";

// looking at the screen, then away from it
const WORK: Duration = Duration::from_secs(20 * 60);
const REST: Duration = Duration::from_secs(20);

const TICK: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Work,
    Rest,
}

const PHASES: [(Phase, Duration); 2] = [(Phase::Work, WORK), (Phase::Rest, REST)];

/*
 * above the volume popup (48 high, 32 up from the bottom), so the two never
 * cover each other
 */
const BOTTOM: i32 = 96;

// nerd font material design eye
const ICON_EYE: &str = "\u{f0208}";

// a break on: the time left in words and how much of it is left, for the popup
#[derive(Debug, Clone, PartialEq)]
pub struct Rest {
    left: String,
    fraction: f32,
}

pub struct EyeBreaks {
    enabled: bool,

    // only while a break is on
    resting: Option<Rest>,

    // the system panel's line beside the checkbox; empty while off
    countdown: String,
}

/*
 * the running cycle, None while off; outside the service so the lock screen's
 * view can restart it without writing a service
 */
static CYCLE: Mutex<Option<Cycle<Phase>>> = Mutex::new(None);

impl Service for EyeBreaks {
    // a file of a few bytes, quick enough for the first view that asks
    fn new() -> Self {
        let saved = fs::read_to_string(state::path(STATE)).unwrap_or_default();
        let enabled = enabled_from(&saved);

        Self {
            enabled,
            resting: None,
            countdown: if enabled {
                countdown(Phase::Work, WORK)
            } else {
                String::new()
            },
        }
    }

    fn interval() -> Duration {
        TICK
    }

    /*
     * ticks the cycle and writes only what views show when it changed: the
     * panel's line once a minute (each second in the last one), the popup each
     * second of a break; the other ticks redraw nothing
     */
    fn listen() {
        loop {
            thread::sleep(Self::interval());

            // one lock at a time: this service, then the cycle, then this service again
            let enabled = EyeBreaks::read().enabled;

            let (resting, countdown) = tick(enabled, Instant::now());

            let changed = {
                let breaks = EyeBreaks::read();

                breaks.resting != resting || breaks.countdown != countdown
            };

            if changed {
                let mut breaks = EyeBreaks::write();

                breaks.resting = resting;
                breaks.countdown = countdown;
            }
        }
    }
}

impl EyeBreaks {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn countdown(&self) -> &str {
        &self.countdown
    }

    /*
     * from a click or key, never inside a view (it writes the service);
     * switching on starts a fresh 20 minutes, switching off ends a break on
     * screen. a file that can't be written only costs remembering it
     */
    pub fn set(enabled: bool) {
        *CYCLE.lock().unwrap_or_else(PoisonError::into_inner) = None;

        let (resting, countdown) = tick(enabled, Instant::now());

        {
            let mut breaks = EyeBreaks::write();

            breaks.enabled = enabled;
            breaks.resting = resting;
            breaks.countdown = countdown;
        }

        let text = if enabled { ON } else { OFF };

        if let Err(error) = state::save(STATE, text) {
            eprintln!("eye breaks: couldn't save the setting: {error}");
        }
    }

    pub fn toggle() {
        let enabled = EyeBreaks::read().enabled;

        EyeBreaks::set(!enabled);
    }
}

/*
 * moves the cycle on (starting it when switched on, dropping it when off)
 * and gives what to show; takes no service lock, only the cycle's
 */
fn tick(enabled: bool, now: Instant) -> (Option<Rest>, String) {
    let mut cycle = CYCLE.lock().unwrap_or_else(PoisonError::into_inner);

    if !enabled {
        *cycle = None;

        return (None, String::new());
    }

    let cycle = cycle.get_or_insert_with(|| Cycle::new(&PHASES, now));

    cycle.advance(now);

    let phase = cycle.phase();
    let left = cycle.left(now);

    let resting = (phase == Phase::Rest).then(|| Rest {
        left: timer::remaining(left),
        fraction: cycle.fraction_left(now),
    });

    (resting, countdown(phase, left))
}

/*
 * the next break starts 20 minutes from now; called by the lock screen's view
 * on every draw, so it only touches the cycle, which is safe inside a view.
 * a break on screen ends with the next tick
 */
pub fn restart_cycle() {
    let mut cycle = CYCLE.lock().unwrap_or_else(PoisonError::into_inner);

    if let Some(cycle) = cycle.as_mut() {
        cycle.restart(Instant::now());
    }
}

// starts the cycle from the start; a service only listens once something reads it
pub fn start() {
    drop(EyeBreaks::read());
}

// only "on" turns them on, so a missing or broken file leaves them off
fn enabled_from(saved: &str) -> bool {
    saved.trim().eq_ignore_ascii_case(ON)
}

// "next in 14 min", "next in 45 s", or "resting · 12 s" during a break
fn countdown(phase: Phase, left: Duration) -> String {
    match phase {
        Phase::Work => format!("next in {}", timer::remaining(left)),
        Phase::Rest => format!("resting · {}", timer::remaining(left)),
    }
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let t = theme();

    let resting = EyeBreaks::read().resting.clone();

    let content = match resting {
        Some(rest) if panels::is_focused(monitor) => timer::card(
            &t,
            ICON_EYE,
            "Rest your eyes",
            "Look at something far away",
            &rest.left,
            rest.fraction,
        ),
        _ => Rectangle::new()
            .width(Parent)
            .height(Parent)
            .fill(Color::TRANSPARENT),
    };

    LayerWindow::new()
        .width(Full)
        .height(timer::CARD_HEIGHT)
        .anchor_vertical(Vertical::Bottom)
        .margin(Margin {
            top: 0,
            right: 0,
            bottom: BOTTOM,
            left: 0,
        })
        .layer(Layer::Overlay)
        .keyboard(Keyboard::None)
        .namespace("amane-eye-breaks")
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .align_child(Center, Center)
                .child(content),
        )
        .click_through()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_on_turns_eye_breaks_on() {
        assert!(enabled_from("on"));
        assert!(enabled_from(" ON\n"));
        assert!(!enabled_from("off"));
        assert!(!enabled_from(""));
        assert!(!enabled_from("garbage"));
    }

    #[test]
    fn the_countdown_says_what_comes_next() {
        assert_eq!(countdown(Phase::Work, WORK), "next in 20 min");
        assert_eq!(
            countdown(Phase::Work, Duration::from_secs(13 * 60 + 1)),
            "next in 14 min"
        );
        assert_eq!(
            countdown(Phase::Work, Duration::from_secs(45)),
            "next in 45 s"
        );
        assert_eq!(
            countdown(Phase::Rest, Duration::from_secs(12)),
            "resting · 12 s"
        );
    }
}
