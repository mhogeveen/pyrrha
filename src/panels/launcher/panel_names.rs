/*
 * the launcher's : mode: the panels it can open, found by name or by what
 * they're for (PanelDef::description)
 */

use crate::panels::list;
use crate::panels::{PanelDef, launchable};

/*
 * an empty search lists every panel in the registry's order; a search ranks by
 * match on the name, or on what it's for, below a name match
 */
pub fn matching(query: &str) -> Vec<&'static PanelDef> {
    if query.is_empty() {
        return launchable().collect();
    }

    // equal scores keep the registry's order
    list::ranked(launchable(), |panel| {
        list::name_or_description(query, panel.name, panel.description)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::{LAUNCHER, NETWORK, SCREENSHOT, WALLPAPER};

    #[test]
    fn panels_rank_by_name_then_description() {
        assert_eq!(matching("wall")[0].name, WALLPAPER);
        assert_eq!(matching("wallpapers")[0].name, WALLPAPER);
        assert_eq!(matching("wifi")[0].name, NETWORK);
    }

    #[test]
    fn panels_never_list_the_launcher_or_screenshot_menu() {
        let names: Vec<&str> = matching("").iter().map(|panel| panel.name).collect();

        assert!(!names.contains(&LAUNCHER));
        assert!(!names.contains(&SCREENSHOT));
        assert!(names.contains(&WALLPAPER));
    }
}
