/*
 * the launcher's plain search: the installed apps, ranked by how well they
 * match and how often you start them (usage.rs)
 */

use amane::DesktopApp;

use crate::panels::list::{self, fuzzy};
use crate::usage::Usage;

// an app counts as used once its score is above this, for the empty search's top
const USED: f64 = 0.1;

// how far a match on the program an app runs ranks below the same match on its name
const PROGRAM_PENALTY: i32 = 10;

/*
 * an empty search puts the apps you use first, most used on top, then the rest
 * alphabetically; a search ranks by match, with use as a bonus that settles
 * close matches but never lifts a poor match above a good one
 */
pub fn sorted<'a>(apps: &'a [DesktopApp], usage: &Usage, query: &str) -> Vec<&'a DesktopApp> {
    let query = query.trim();

    if query.is_empty() {
        let mut used: Vec<(f64, &DesktopApp)> = apps
            .iter()
            .map(|app| (usage.score(app.name()), app))
            .filter(|(score, _)| *score > USED)
            .collect();

        used.sort_by(|a, b| b.0.total_cmp(&a.0));

        let rest = apps.iter().filter(|app| usage.score(app.name()) <= USED);

        return used.into_iter().map(|(_, app)| app).chain(rest).collect();
    }

    // equal scores stay alphabetical
    list::ranked(apps, |app| {
        let bonus = use_bonus(usage.score(app.name()));

        score_app(app, query).map(|score| score + bonus)
    })
}

// grows with use but slows down, and stops at 24 (about two letters' worth of match)
fn use_bonus(score: f64) -> i32 {
    (8.0 * score.ln_1p()).min(24.0) as i32
}

// the name counts most, the command it runs helps find things like "code"
fn score_app(app: &DesktopApp, query: &str) -> Option<i32> {
    let name = fuzzy(query, app.name());

    let program = app
        .exec()
        .split_whitespace()
        .next()
        .and_then(|command| command.rsplit('/').next())
        .and_then(|program| fuzzy(query, program))
        .map(|score| score - PROGRAM_PENALTY);

    name.max(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_bonus_grows_slowly_and_stops() {
        assert_eq!(use_bonus(0.0), 0);
        assert!(use_bonus(1.0) > 0);
        assert!(use_bonus(10.0) > use_bonus(1.0));
        assert_eq!(use_bonus(1_000_000.0), 24);
    }
}
