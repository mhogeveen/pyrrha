/*
 * pyrrha: a desktop shell for mango on the amane library. this starts the
 * parts that must run from launch, registers the ipc handlers (ipc.rs), and makes every
 * window once per monitor; the order of the windows is their stacking order,
 * bottom first: wallpaper, bar, backdrop, popups (notifications, volume, eye
 * breaks), every panel, the screenshot
 * menu (over the panels it can capture), the lock screen
 */

use amane::App;

mod animations;
mod audio;
mod bar;
mod bluetooth;
mod brightness;
mod calculator;
mod calendar;
mod clipboard_preview;
mod clock;
mod command;
mod covers;
mod desktop;
mod doctor;
mod eye_breaks;
mod health;
mod idle;
mod ipc;
mod json;
mod keybinds;
mod layout;
mod lock;
mod network;
mod night_light;
mod notifications;
mod osd;
mod panels;
mod recording;
mod screenshot;
mod session;
mod settings;
mod state;
mod system;
mod theme;
mod timer;
mod usage;
mod user_unit;
mod vpn;
mod wallpaper;
mod wallpaper_copies;
mod weather;

fn main() {
    // `amane-shell doctor` checks the host instead of starting the shell
    if std::env::args().nth(1).as_deref() == Some("doctor") {
        std::process::exit(doctor::run());
    }

    // moves the state folders from before the rename, before anything writes them
    state::migrate();

    // locks on suspend and `loginctl lock-session`
    session::watch_logind();

    /*
     * answers BlueZ when a device asks to confirm a pairing code, and registers
     * again when bluetoothd restarts
     */
    bluetooth::agent::start();
    bluetooth::start();

    // records the clipboard from the start, the panel only reads it when open
    panels::clipboard::start();

    // restores the notifications from before a restart, and keeps saving them
    notifications::history::start();

    // the volume popup follows every change from the start
    osd::start();

    // the 20 minutes to the next eye break count from launch, when switched on
    eye_breaks::start();

    // warns once about a hot cpu or gpu, a nearly full disk or a failed unit
    health::start();

    // every command in ipc's table, which the launcher's ! mode lists too
    let app = ipc::COMMANDS
        .iter()
        .fold(App::new().font("BlexMono Nerd Font"), |app, command| {
            app.ipc(command.name, command.handler)
        });

    app
        // first, so it is the bottom-most window
        .window_per_monitor(wallpaper::view)
        .window_per_monitor(bar::view)
        .window_per_monitor(panels::backdrop::view)
        .window_per_monitor(notifications::popups::view)
        .window_per_monitor(osd::view)
        .window_per_monitor(eye_breaks::view)
        // under the panels, so a panel opened while recording a region covers the frame
        .window_per_monitor(recording::frame_view)
        .window_per_monitor(panels::launcher::view)
        .window_per_monitor(panels::clipboard::view)
        .window_per_monitor(panels::session::view)
        .window_per_monitor(panels::layout::view)
        .window_per_monitor(panels::notifications::view)
        .window_per_monitor(panels::network::view)
        .window_per_monitor(panels::bluetooth::view)
        .window_per_monitor(panels::audio::view)
        .window_per_monitor(panels::system::view)
        .window_per_monitor(panels::calendar::view)
        .window_per_monitor(panels::wallpaper::view)
        .window_per_monitor(panels::keybinds::view)
        // after every panel, so it opens over one left open underneath
        .window_per_monitor(panels::screenshot::view)
        .lock(lock::view)
        .run();
}
