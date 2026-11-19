# Pyrrha: context for agents

Pyrrha, a desktop shell for the MangoWM Wayland compositor, in Rust on the
[amane](https://github.com/MystiaFin/amane) library. Read this before changing
anything. More detail, read when the task touches it:

- `docs/modules.md`: what each file in `src/` does
- `docs/subsystems.md`: how services, panels, notifications, calendar,
  wallpaper, lock, popups, brightness and VPN work
- `docs/amane.md`: amane's limits (keys, D-Bus, images, widgets, layers)
- `docs/machine.md`: this machine (monitor, keyboard, audio, mango binds)
- `README.md`: installing on a new machine, the dependencies, and the
  per-host settings (`src/settings.rs`); update it when a dependency or a
  setting changes
- `docs/recommendations.md` (open items and the "Already decided" list, read before
  proposing anything), `docs/done.md` (built, by number), `docs/history.md` (why)

## Where things live
- This folder is `~/git/pyrrha`, its own git repository; `~/.config/amane`
  links here (home-manager, `~/.nixos/home/packages.nix`). Open
  files by the real path.
- `amane compile` builds `src/` in `~/.cache/amane/project` (library in
  `~/.cache/amane/library`, overwritten each compile, so don't patch it).
  No crates: only `std`, `amane`, and libc via `unsafe extern "C"`.
- `Cargo.toml` here is for the editor; `amane compile` rewrites it on every
  build (its `[[bin]] path` comes back absolute, through `~/.config/amane`),
  so edits to it don't stay.
- Per-machine values (programs, units, gpu, sensors, folders, weather place):
  `Settings::default()` in `src/settings.rs`, a host changing only what it
  needs in `src/settings/<hostname>.rs` (compiled; picked by hostname).
- State: `~/.local/state/pyrrha/` (survives reboots),
  `$XDG_RUNTIME_DIR/pyrrha/` (until logout).
- Mango config: `~/.nixos/home/mango.nix` (home-manager writes
  `~/.config/mango/config.conf` from it; `~/.nixos/config/mango/` is unused,
  edits there do nothing). NixOS
  host: `~/.nixos/hosts/prometheus/default.nix`; home-manager: `~/.nixos/home/`.

## Workflow
- Build `amane compile` (zero warnings, fix every one); format
  `rustfmt --edition 2024 src/main.rs`; test `./test.sh` (`./test.sh system::`
  filters; 280 tests), add tests for pure logic.
- Name: the shell is Pyrrha, but the binary and process stay `amane-shell`
  (amane generates the Cargo package and picks that name); state folders are
  `pyrrha/`, moved from `amane-shell/` once at start (`state::migrate`).
- Check a host `~/.cache/amane/amane-shell doctor` (runs instead of the shell;
  lists missing programs, units, services; exit 1 when a required one is missing).
- Restart `env -u LD_LIBRARY_PATH amane restart` after a compile (loses panel
  state; fine for checks). The shell passes its environment to every app it
  launches, and Claude Code's wrapper sets `LD_LIBRARY_PATH` to unstable's
  alsa-lib (needs glibc 2.43), so stable apps like 1Password die on start
  (`docs/amane.md`).
- IPC `amane ipc call <name> ...`: `panel open|close|toggle <name>`,
  `close-all`, `list`; `lock`; `accent [next|<name>]`;
  `weather [refresh|location <lat> <lon> <name>]`; `media play-pause|next|previous`;
  `brightness up|down`; `record stop|menu` (menu: stops a recording, else
  toggles the screenshot menu); `eye-breaks skip` (ends the break on
  screen, the next in 20 minutes). All listed in `src/ipc.rs` (`COMMANDS`), which
  registers them; the launcher's `!` calls them too (`!accent next`).
- Screenshots: `grim -g "x,y wxh"` takes logical coordinates (amane's widget
  units); the png comes out at physical size (× scale). On the AOC U27B3AF
  (`docs/machine.md`; 2560×1440 logical, scale 1.5): top-right panels
  `"2140,0 420x760"`; `grim -s 0.5` for the whole screen. On another
  monitor, redo these from `mmsg get all-monitors`. Fresh file names, `sleep 10` and `ls`
  before reading. Keys: `wt=$(nix build --no-link --print-out-paths
  nixpkgs#wtype)/bin/wtype; $wt '?'`.
- Leave the machine as it was: volume, mute, brightness
  (`ddcutil --bus 5 getvcp 10 --brief`), wallpaper. Ask before locking, the
  microphone, starting media, or touching the user's files.
  `media play-pause` toggles whatever player is active.
- External programs: `command::read` (output on success) or `command::capture`
  (the full output) with a time limit,
  `command::feed(child, input)` for stdin; `clock::local_now()` for the time,
  never `date`.

## Working agreements
- Plan mode: only read and plan; ask with options (recommended first); wait
  for "Go ahead".
- Scope: edit only this folder unless asked or agreed in a plan; the user
  runs `nixos-rebuild`. A new file a flake must see: `git add --intent-to-add`.
- Git: the user commits; don't commit, push or stage. Check `git status` /
  `git log` first; don't revert the user's changes.
- Recommendations: keep statuses current; built items move to `docs/done.md` with
  their number; new ideas get the next number (55 was the last).
- Code style: lowercase comments, `/* */` blocks, `//` lines, saying why;
  every module starts with a block comment; descriptive names, no
  abbreviations; constants for every size and timing.
- Nerd Font glyphs: look them up in nerd-fonts' `glyphnames.json`, never guess.

## Rules that break things when ignored
- Services (`X::read()`/`X::write()`): never write one inside a view (the view
  holds a read lock: deadlock); start a thread or set a static instead. Never
  write while holding any guard, and never read a service whose guard you
  already hold (copy out, let go, then read the next); views may hold several
  different reads at once. A service's `listen()` starts on first read.
- Layer surfaces: showing, hiding or resizing one while another changes can
  crash the shell (`Protocol error 0 on zwlr_layer_surface_v1`). Panels have
  fixed sizes and show or hide one at a time, `SWAP_DELAY` apart
  (`hide_time`, `quiet_at` in `src/panels/mod.rs`); popups stay mapped and
  never resize. No windows that show and hide quickly.
- No `Canvas` (arcs, lines, paths) and no `.rotate()`: both go through
  vello, whose compute shaders hang this GPU
  (amdgpu "ring gfx timeout", GPU reset, shell gone). Build shapes from
  `Rectangle`s; after any new kind of drawing, check
  `journalctl -k | grep -i "gfx timeout"` (`docs/amane.md`).
- Panels: one open at a time (all in `PANELS`), except the screenshot menu;
  fixed size, reserve space rather than resize.
- Keys in panels: no modifiers or key repeat; Delete, PgUp/PgDn, F-keys are
  `Key::Other`; Shift arrives as an uppercase character; a focused
  `TextInput` takes every character.
- UI, decided: the accent picked in the system panel for every highlight,
  warning and error (no status colors; the exceptions are `theme.error`, only for the bar's
  privacy dot and recording time, and `theme.warning` (#f4a21c), only for
  the bar's idle-off dot); font `BlexMono Nerd Font`
  (monospaced: width = chars × size × 0.6); no Do Not Disturb.
