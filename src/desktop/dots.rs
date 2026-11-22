/*
 * dots: dot paper, a grid of small round dots over the page below the bar,
 * centred so the margins match on both sides, with the art designs' small
 * time in the corner and no corner ticks. each dot gets its own faint shade,
 * mostly near the darkest and a few a little brighter, like ink that took
 * unevenly; the shades come from a fixed seed, so the sheet is the same every
 * day and in its thumbnail. one dot is in the accent: the time, walking
 * through the sheet in reading order from midnight at the top left to the end
 * of the day at the bottom right (time_dot)
 *
 * a row of dots is one Row and the rows one Column, so the page stays a few
 * dozen widgets deep rather than one placed widget per dot. dots are rounded
 * rectangles, which stay off vello (docs/amane.md)
 */

use amane::{Color, Column, Full, Rectangle, Row, Widget};

use super::{Moment, Page, Random, placed, signature};
use crate::theme::Theme;

// from one dot's centre to the next, and a dot's size, at the reference height
const PITCH: f32 = 40.0;
const DOT: f32 = 3.0;

// the shades' seed: fixed, so the same dots are bright every day
const SEED: u64 = 0xD075_D075_D075_D075;

/*
 * a dot's shade runs from bg2 (0) to bg4 (1): the faintest and brightest
 * here, squared in between so most dots stay near the faint end
 */
const FAINTEST: f32 = 0.1;
const BRIGHTEST: f32 = 0.8;

pub fn layers(page: Page, moment: &Moment, t: &Theme) -> Vec<Box<dyn Widget>> {
    let pitch = page.scaled(PITCH);
    let dot = page.scaled(DOT);
    let gap = (pitch - dot).max(0.0);

    let (columns, left) = grid(page.width, pitch, dot);
    let (rows, top) = grid(page.body_height(), pitch, dot);

    // the same sequence for the page and the thumbnail, dot by dot from the top left
    let mut random = Random(SEED);

    // the one dot in the accent: how far through the day it is, in reading order
    let now = time_dot(moment.hour, moment.minute, rows * columns);

    let sheet: Vec<Box<dyn Widget>> = (0..rows)
        .map(|row_index| -> Box<dyn Widget> {
            let row: Vec<Box<dyn Widget>> = (0..columns)
                .map(|column_index| -> Box<dyn Widget> {
                    // drawn for every dot, so the others keep their shades whatever the time
                    let amount = shade(random.unit());

                    let fill = if row_index * columns + column_index == now {
                        t.accent
                    } else {
                        mix(t.bg2, t.bg4, amount)
                    };

                    Box::new(
                        Rectangle::new()
                            .width(dot)
                            .height(dot)
                            .radius(Full)
                            .fill(fill),
                    )
                })
                .collect();

            Box::new(Row::new(row).gap(gap))
        })
        .collect();

    vec![
        Box::new(placed(left, page.top + top, Column::new(sheet).gap(gap))),
        Box::new(signature(page, moment, t)),
    ]
}

/*
 * how many dots fit along a length, a pitch apart with a margin of at least
 * half a pitch at each end, and where the first one starts so both margins
 * are the same
 */
fn grid(length: f32, pitch: f32, dot: f32) -> (usize, f32) {
    if pitch <= 0.0 || length < pitch {
        return (0, 0.0);
    }

    // the dots' centres span (count - 1) pitches, inside length minus a pitch of margin
    let count = ((length - pitch) / pitch).floor() as usize + 1;
    let span = (count - 1) as f32 * pitch + dot;

    (count, (length - span) / 2.0)
}

const MINUTES_PER_DAY: usize = 24 * 60;

// which of the dots (counted in reading order) stands for this minute: midnight the first, 23:59 near the last
fn time_dot(hour: u32, minute: u32, dots: usize) -> usize {
    let minutes = (hour as usize * 60 + minute as usize).min(MINUTES_PER_DAY - 1);

    minutes * dots / MINUTES_PER_DAY
}

// a random 0 to 1 as a dot's shade between FAINTEST and BRIGHTEST, leaning faint
fn shade(random: f32) -> f32 {
    FAINTEST + (BRIGHTEST - FAINTEST) * random * random
}

// from one colour (0) to another (1), channel by channel
fn mix(from: Color, to: Color, amount: f32) -> Color {
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;

    Color::rgb(
        channel(from.red(), to.red()),
        channel(from.green(), to.green()),
        channel(from.blue(), to.blue()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_fit_with_even_margins() {
        // the body below the bar on this monitor
        let (count, start) = grid(1400.0, 40.0, 4.0);

        assert_eq!(count, 35);
        let end = 1400.0 - (start + (count - 1) as f32 * 40.0 + 4.0);
        assert!((start - end).abs() < 1e-3);
        assert!(start >= 20.0 - 4.0);

        let (count, start) = grid(2560.0, 40.0, 4.0);

        assert_eq!(count, 64);
        assert!(start > 0.0);
    }

    #[test]
    fn too_small_for_a_dot() {
        assert_eq!(grid(30.0, 40.0, 4.0), (0, 0.0));
        assert_eq!(grid(100.0, 0.0, 4.0), (0, 0.0));
    }

    #[test]
    fn the_time_walks_through_the_dots() {
        assert_eq!(time_dot(0, 0, 2240), 0);
        assert_eq!(time_dot(12, 0, 2240), 1120);
        assert!(time_dot(23, 59, 2240) < 2240);
        assert_eq!(time_dot(12, 0, 0), 0);
    }

    #[test]
    fn shades_stay_faint_and_lean_dark() {
        assert_eq!(shade(0.0), FAINTEST);
        assert!((shade(1.0) - BRIGHTEST).abs() < 1e-6);
        assert!(shade(0.5) < (FAINTEST + BRIGHTEST) / 2.0);
    }

    #[test]
    fn colours_mix_by_channel() {
        let from = Color::rgb(0x20, 0x00, 0xff);
        let to = Color::rgb(0x40, 0xff, 0x00);

        assert_eq!(mix(from, to, 0.0).hex(), from.hex());
        assert_eq!(mix(from, to, 1.0).hex(), to.hex());
        assert_eq!(mix(from, to, 0.5).green(), 0x80);
    }
}
