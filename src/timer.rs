/*
 * the pieces any timer in the shell is made of, kept apart from what a timer
 * is for: a countdown, a cycle of countdowns that repeats (eye breaks today;
 * a pomodoro's focus and break later), the time left in words, and the card a
 * timer's popup shows
 *
 * no services and no threads here: whoever runs a timer ticks it from its own
 * loop and decides when to redraw, so this part stays pure and tested
 */

use std::time::{Duration, Instant};

use amane::{
    Align::{Center, Start},
    Column, Rectangle, Row,
    Size::Parent,
    Text, Weight, children,
};

use crate::panels::ui::{self, BUTTON, PADDING};
use crate::theme::Theme;

const SECONDS_PER_MINUTE: u64 = 60;
const MINUTES_PER_HOUR: u64 = 60;

// a length of time counting down from when it was started
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Countdown {
    started: Instant,
    length: Duration,
}

impl Countdown {
    pub fn new(now: Instant, length: Duration) -> Self {
        Self {
            started: now,
            length,
        }
    }

    // nothing left once it's over, however long ago that was
    pub fn left(&self, now: Instant) -> Duration {
        self.length
            .saturating_sub(now.saturating_duration_since(self.started))
    }

    pub fn is_over(&self, now: Instant) -> bool {
        self.left(now).is_zero()
    }

    // 1 when just started, 0 when over; for a meter that empties
    pub fn fraction_left(&self, now: Instant) -> f32 {
        if self.length.is_zero() {
            return 0.0;
        }

        self.left(now).as_secs_f32() / self.length.as_secs_f32()
    }

    pub fn ends_at(&self) -> Instant {
        self.started + self.length
    }
}

/*
 * phases that follow each other and start over after the last, each a
 * countdown of its own length: eye breaks are 20 minutes of work then 20
 * seconds of rest, a pomodoro 25 minutes of focus then 5 of break
 */
#[derive(Debug, Clone, Copy)]
pub struct Cycle<P: Copy + 'static> {
    phases: &'static [(P, Duration)],
    index: usize,
    countdown: Countdown,
}

impl<P: Copy + 'static> Cycle<P> {
    // starts in the first phase; a cycle needs at least one
    pub fn new(phases: &'static [(P, Duration)], now: Instant) -> Self {
        assert!(!phases.is_empty(), "a cycle needs at least one phase");

        Self {
            phases,
            index: 0,
            countdown: Countdown::new(now, phases[0].1),
        }
    }

    pub fn phase(&self) -> P {
        self.phases[self.index].0
    }

    pub fn left(&self, now: Instant) -> Duration {
        self.countdown.left(now)
    }

    pub fn fraction_left(&self, now: Instant) -> f32 {
        self.countdown.fraction_left(now)
    }

    /*
     * moves on to the phase that's running now and gives it when that's a
     * different one. each phase starts where the one before ended, so a late
     * tick doesn't stretch the cycle; after a gap longer than whole cycles
     * those are skipped at once rather than stepped through
     */
    pub fn advance(&mut self, now: Instant) -> Option<P> {
        if !self.countdown.is_over(now) {
            return None;
        }

        let whole: Duration = self.phases.iter().map(|(_, length)| *length).sum();

        // phases of no length would never end; keep the last one
        if whole.is_zero() {
            return None;
        }

        let behind = now.saturating_duration_since(self.countdown.ends_at());
        let skipped_cycles = behind.as_nanos() / whole.as_nanos();
        let skipped = whole.saturating_mul(u32::try_from(skipped_cycles).unwrap_or(u32::MAX));

        let mut start = self.countdown.ends_at() + skipped;
        let mut index = self.index;

        loop {
            index = (index + 1) % self.phases.len();

            let countdown = Countdown::new(start, self.phases[index].1);

            if !countdown.is_over(now) {
                let entered = index != self.index;

                self.index = index;
                self.countdown = countdown;

                return entered.then(|| self.phase());
            }

            start = countdown.ends_at();
        }
    }

    // back to the start of the first phase
    pub fn restart(&mut self, now: Instant) {
        self.index = 0;
        self.countdown = Countdown::new(now, self.phases[0].1);
    }
}

/*
 * the time left in words: "45 s" under a minute, "14 min" under an hour,
 * "1 h 05 min" after; rounded up, so it never says 0 while time is left and
 * reads the full length at the start
 */
pub fn remaining(left: Duration) -> String {
    let seconds = left.as_secs() + u64::from(left.subsec_nanos() > 0);

    if seconds < SECONDS_PER_MINUTE {
        return format!("{seconds} s");
    }

    let minutes = seconds.div_ceil(SECONDS_PER_MINUTE);

    if minutes < MINUTES_PER_HOUR {
        return format!("{minutes} min");
    }

    format!(
        "{} h {:02} min",
        minutes / MINUTES_PER_HOUR,
        minutes % MINUTES_PER_HOUR
    )
}

// the popup card's size, so the window around it can be fixed to fit
pub const CARD_WIDTH: f32 = 360.0;
pub const CARD_HEIGHT: f32 = PADDING * 2.0 + CARD_TOP + CARD_GAP + BUTTON;

// the icon and the two lines of text, then the meter under them
const CARD_TOP: f32 = 40.0;
const CARD_GAP: f32 = 8.0;
const CARD_ICON: f32 = 32.0;
const CARD_ICON_SIZE: f32 = 22.0;
const CARD_LEFT_TEXT: f32 = 64.0;
const CARD_METER: f32 = CARD_WIDTH - PADDING * 2.0 - CARD_LEFT_TEXT - CARD_GAP;
const CARD_BORDER: f32 = 1.0;

/*
 * a timer's popup: the icon in the accent, a title and a line under it, then
 * a meter emptying as the time runs out and the time left; the same look as
 * the volume popup
 */
pub fn card(
    t: &Theme,
    icon: &str,
    title: &str,
    line: &str,
    left: &str,
    fraction: f32,
) -> Rectangle {
    let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as u8;

    let words = Column::new(children![
        Text::new(title)
            .size(ui::TEXT_BODY)
            .weight(Weight::Bold)
            .color(t.fg)
            .elide(),
        Text::new(line).size(ui::TEXT_SMALL).color(t.fg_dim).elide(),
    ])
    .gap(2.0);

    let top = Row::new(children![
        Rectangle::new()
            .width(CARD_ICON)
            .height(CARD_TOP)
            .align_child(Start, Center)
            .child(Text::new(icon).size(CARD_ICON_SIZE).color(t.accent)),
        Rectangle::new()
            .width(Parent)
            .height(CARD_TOP)
            .align_child(Start, Center)
            .child(words),
    ])
    .gap(CARD_GAP)
    .align(Center)
    .height(CARD_TOP);

    let bottom = Row::new(children![
        ui::meter(t, CARD_METER, percent, false),
        Rectangle::new()
            .width(CARD_LEFT_TEXT)
            .height(BUTTON)
            .align_child(Start, Center)
            .child(Text::new(left).size(ui::TEXT_NOTE).color(t.fg)),
    ])
    .gap(CARD_GAP)
    .align(Center)
    .height(BUTTON);

    Rectangle::new()
        .width(CARD_WIDTH)
        .height(CARD_HEIGHT)
        .fill(t.bg_deep)
        .border(CARD_BORDER, t.bg3)
        .padding(PADDING)
        .child(Column::new(children![top, bottom]).gap(CARD_GAP))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);
    const SECOND: Duration = Duration::from_secs(1);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Phase {
        Work,
        Rest,
    }

    const PHASES: [(Phase, Duration); 2] = [
        (Phase::Work, Duration::from_secs(20 * 60)),
        (Phase::Rest, Duration::from_secs(20)),
    ];

    #[test]
    fn a_countdown_runs_out_and_stays_out() {
        let start = Instant::now();
        let countdown = Countdown::new(start, MINUTE);

        assert_eq!(countdown.left(start), MINUTE);
        assert_eq!(countdown.fraction_left(start), 1.0);
        assert_eq!(countdown.left(start + 15 * SECOND), 45 * SECOND);
        assert!(!countdown.is_over(start + 59 * SECOND));
        assert!(countdown.is_over(start + MINUTE));
        assert_eq!(countdown.left(start + 10 * MINUTE), Duration::ZERO);
        assert_eq!(countdown.fraction_left(start + 10 * MINUTE), 0.0);
    }

    #[test]
    fn a_cycle_moves_through_its_phases_and_starts_over() {
        let start = Instant::now();
        let mut cycle = Cycle::new(&PHASES, start);

        assert_eq!(cycle.phase(), Phase::Work);
        assert_eq!(cycle.advance(start + 19 * MINUTE), None);

        let rest = start + 20 * MINUTE;

        assert_eq!(cycle.advance(rest), Some(Phase::Rest));
        assert_eq!(cycle.left(rest), 20 * SECOND);
        assert_eq!(cycle.advance(rest + 10 * SECOND), None);
        assert_eq!(cycle.advance(rest + 20 * SECOND), Some(Phase::Work));
        assert_eq!(cycle.left(rest + 20 * SECOND), 20 * MINUTE);
    }

    #[test]
    fn a_late_tick_keeps_the_rhythm() {
        let start = Instant::now();
        let mut cycle = Cycle::new(&PHASES, start);

        // checked 5 s after work ended: the rest started then, 15 s are left
        let late = start + 20 * MINUTE + 5 * SECOND;

        assert_eq!(cycle.advance(late), Some(Phase::Rest));
        assert_eq!(cycle.left(late), 15 * SECOND);
    }

    #[test]
    fn a_long_gap_skips_whole_cycles() {
        let start = Instant::now();
        let mut cycle = Cycle::new(&PHASES, start);

        // three whole cycles and ten minutes later: back in work, ten minutes left
        let whole = 20 * MINUTE + 20 * SECOND;
        let later = start + 3 * whole + 10 * MINUTE;

        assert_eq!(cycle.advance(later), None);
        assert_eq!(cycle.phase(), Phase::Work);
        assert_eq!(cycle.left(later), 10 * MINUTE);
    }

    #[test]
    fn a_restart_goes_back_to_the_first_phase() {
        let start = Instant::now();
        let mut cycle = Cycle::new(&PHASES, start);
        let rest = start + 20 * MINUTE;

        cycle.advance(rest);
        cycle.restart(rest + 5 * SECOND);

        assert_eq!(cycle.phase(), Phase::Work);
        assert_eq!(cycle.left(rest + 5 * SECOND), 20 * MINUTE);
    }

    #[test]
    fn the_time_left_reads_rounded_up() {
        assert_eq!(remaining(Duration::ZERO), "0 s");
        assert_eq!(remaining(Duration::from_millis(300)), "1 s");
        assert_eq!(remaining(45 * SECOND), "45 s");
        assert_eq!(remaining(59 * SECOND), "59 s");
        assert_eq!(remaining(MINUTE), "1 min");
        assert_eq!(remaining(61 * SECOND), "2 min");
        assert_eq!(remaining(20 * MINUTE), "20 min");
        assert_eq!(
            remaining(20 * MINUTE - Duration::from_millis(500)),
            "20 min"
        );
        assert_eq!(remaining(60 * MINUTE), "1 h 00 min");
        assert_eq!(remaining(65 * MINUTE), "1 h 05 min");
    }
}
