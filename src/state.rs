/*
 * small files the shell remembers things in between restarts, like the accent
 * and how often apps are launched: $XDG_STATE_HOME/pyrrha/<name>, or
 * ~/.local/state/pyrrha/<name>, outside the config repo so they never
 * dirty git
 */

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// the shell's own folder, in the state and in the runtime folder
const FOLDER: &str = "pyrrha";

// the folder's name from before the shell was called pyrrha, moved over once
const OLD_FOLDER: &str = "amane-shell";

// $XDG_STATE_HOME, or ~/.local/state when it isn't set to an absolute path
fn state_dir() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();

            home.join(".local").join("state")
        })
}

pub fn path(name: &str) -> PathBuf {
    state_dir().join(FOLDER).join(name)
}

/*
 * the session's runtime folder ($XDG_RUNTIME_DIR, in memory, emptied at
 * logout); None when it isn't set to an absolute path, then nothing is kept there
 */
pub fn runtime_dir() -> Option<PathBuf> {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

// the shell's own file or folder in the runtime folder: $XDG_RUNTIME_DIR/pyrrha/<name>
pub fn runtime_path(name: &str) -> Option<PathBuf> {
    Some(runtime_dir()?.join(FOLDER).join(name))
}

/*
 * moves the folders from before the rename (amane-shell) to pyrrha, so the
 * accent, weather place and launch counts are kept; runs before any service
 * writes, and a failure only means starting with fresh state
 */
pub fn migrate() {
    migrate_in(&state_dir());

    if let Some(runtime) = runtime_dir() {
        migrate_in(&runtime);
    }
}

// moves base/amane-shell to base/pyrrha, unless pyrrha is already there
fn migrate_in(base: &Path) {
    let old = base.join(OLD_FOLDER);
    let new = base.join(FOLDER);

    if old.is_dir() && !new.exists() {
        let _ = fs::rename(&old, &new);
    }
}

// writes the file, making the folder first when it's missing
pub fn save(name: &str, contents: &str) -> io::Result<()> {
    write_atomically(&path(name), contents)
}

/*
 * written beside the file and moved over it, so a shell stopped halfway leaves
 * the old file rather than an empty or cut-off one
 */
pub fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }

    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".new");

    fs::write(&temporary, contents)?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_replaces_the_file_whole() {
        let folder = std::env::temp_dir().join("pyrrha-state-test");
        let file = folder.join("value");

        write_atomically(&file, "first").expect("written");
        write_atomically(&file, "second").expect("written again");

        assert_eq!(fs::read_to_string(&file).expect("read"), "second");
        assert!(!folder.join("value.new").exists());

        fs::remove_dir_all(&folder).expect("cleaned up");
    }

    #[test]
    fn the_old_folder_moves_over_once() {
        let base = std::env::temp_dir().join("pyrrha-migrate-test");
        let _ = fs::remove_dir_all(&base);

        write_atomically(&base.join(OLD_FOLDER).join("accent"), "blue").expect("written");
        migrate_in(&base);

        assert_eq!(
            fs::read_to_string(base.join(FOLDER).join("accent")).expect("moved"),
            "blue"
        );
        assert!(!base.join(OLD_FOLDER).exists());

        // with both there, the new one is kept as it is
        write_atomically(&base.join(OLD_FOLDER).join("accent"), "red").expect("written");
        migrate_in(&base);

        assert_eq!(
            fs::read_to_string(base.join(FOLDER).join("accent")).expect("kept"),
            "blue"
        );
        assert!(base.join(OLD_FOLDER).exists());

        fs::remove_dir_all(&base).expect("cleaned up");
    }
}
