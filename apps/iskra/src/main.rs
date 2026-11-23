//! The Iskra desktop app: an eframe (winit + wgpu) window around `iskra-ui`.

// A release build on Windows opens no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod graphics;
#[cfg(target_os = "macos")]
mod menu_bar;
mod sound_in;
mod sound_out;
mod tablet;

use iskra_engine::Session;
use iskra_ui::IskraApp;
use iskra_ui::log::Log;
use iskra_ui::prefs::Preferences;

struct Shell {
    app: IskraApp,
    /// Keeps the sound output open while the window lives.
    _sound: Option<cpal::Stream>,
    /// The sound input, while something is recorded.
    input: Option<cpal::Stream>,
    log: Log,
    /// Keeps the AppKit tablet monitor installed while the window lives.
    #[cfg(target_os = "macos")]
    _tablet: Option<photocraft_tablet::macos::Monitor>,
    /// The system's menu bar, with what was chosen in it.
    #[cfg(target_os = "macos")]
    menu: Option<menu_bar::MenuBar>,
    /// The pen read through the tablet driver's WinTab, where it offers it and the preferences
    /// ask for it; the window it is read on; and the choice it was opened for.
    #[cfg(windows)]
    wintab: Option<iskra_tablet_windows::WinTab>,
    #[cfg(windows)]
    hwnd: Option<isize>,
    #[cfg(windows)]
    tablet_input: Option<iskra_ui::prefs::TabletInput>,
}

#[cfg(windows)]
impl Shell {
    /// Read the pen as the preferences say, opening or closing WinTab when they change.
    fn follow_tablet_input(&mut self) {
        let want = self.app.prefs.tablet_input;
        if self.tablet_input == Some(want) {
            return;
        }
        self.tablet_input = Some(want);
        self.app.stylus.ignore_touch = !want.reads_ink();
        self.wintab = None;
        if !want.reads_wintab() {
            self.log.say("pen: read as Windows Ink gives it: pressure alone, as the force of its touch events");
            return;
        }
        let Some(hwnd) = self.hwnd else {
            self.log.say("pen: the window's handle was not given, so WinTab is not read");
            return;
        };
        match iskra_tablet_windows::WinTab::open(hwnd) {
            Ok(wintab) => {
                self.log.say(format!("pen: read through WinTab, from \"{}\": pressure, tilt, rotation and the eraser end", wintab.device));
                self.wintab = Some(wintab);
            }
            Err(why) if want.reads_ink() => {
                self.log.say(format!("pen: not read through WinTab ({why}); pressure comes as the force of Windows Ink's touch events"))
            }
            Err(why) => self.log.say(format!("pen: not read through WinTab ({why}), and Windows Ink is not read as asked: a pen draws as a mouse")),
        }
    }
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        // What was chosen in the system's menu bar is run as the window's own menus run it.
        #[cfg(target_os = "macos")]
        if let Some(menu) = &mut self.menu {
            for command in menu.take_chosen() {
                // Its shortcut, if the window was given the key as well, is not run again.
                if let Some(shortcut) = menu_bar::entry(&self.app.menu_bar(), &command).and_then(|entry| entry.shortcut) {
                    ui.input_mut(|input| input.consume_shortcut(&shortcut));
                }
                self.app.run_menu_command(&command, ui.ctx());
            }
        }
        // The pen as WinTab read it since the last frame, before the app reads the pointer.
        #[cfg(windows)]
        self.follow_tablet_input();
        #[cfg(windows)]
        if let Some(wintab) = &mut self.wintab {
            use iskra_tablet_windows::Taken;
            match wintab.take() {
                Taken::Nothing => {}
                Taken::Away => self.app.stylus.feed.set(None),
                Taken::Pen(pen) => self.app.stylus.feed.set(Some(iskra_ui::stylus::PenSample {
                    pressure: pen.pressure,
                    tilt_x: pen.tilt_x,
                    tilt_y: pen.tilt_y,
                    rotation: pen.rotation,
                    eraser: pen.eraser,
                })),
            }
        }
        self.app.ui(ui);
        // The bar follows the app: what can run, what is on, what Undo undoes, and no
        // shortcuts while a text is typed or a key is pressed for a command in Settings.
        #[cfg(target_os = "macos")]
        if let Some(menu) = &mut self.menu
            && let Err(why) = menu.follow(&self.app.menu_bar(), ui.ctx().text_edit_focused() || self.app.wants_keys())
        {
            // Without a bar that follows it, the window has its own menus again.
            self.log.say(format!("the menu bar could not be made anew: {why}"));
            self.app.set_system_menus(false);
            self.menu = None;
        }
        // The microphone is open for as long as a take is being recorded, and no longer.
        let recorder = self.app.recorder();
        match (recorder.is_recording(), self.input.is_some()) {
            (true, false) => {
                self.input = sound_in::open(recorder.clone(), &self.log);
                if self.input.is_none() {
                    recorder.fail("no sound input could be opened: is there a microphone, and is Iskra allowed to use it?");
                }
            }
            (false, true) => self.input = None,
            _ => {}
        }
    }
}

/// The log, written to a file beside the preferences from the first line on, and told of a
/// panic before the app ends by it: a Windows release build has no console to say it in.
fn begin_log() -> Log {
    let log = Log::default();
    if let Some(preferences) = Preferences::default_path() {
        log.to_file(&preferences.with_file_name("iskra.log"));
    }
    let (said, before) = (log.clone(), std::panic::take_hook());
    std::panic::set_hook(Box::new(move |panic| {
        said.say(format!("the app ended by a panic: {panic}"));
        before(panic);
    }));
    log
}

fn main() -> eframe::Result {
    let log = begin_log();
    let in_app = log.clone();
    let left = Preferences::default_path().map(|path| Preferences::load(&path)).unwrap_or_default();
    // Opened by a project file, as a double click on one does, the app opens it; from its
    // icon, it shows the start window first.
    let file = std::env::args_os().skip(1).map(std::path::PathBuf::from).find(|path| path.extension().is_some_and(|ext| ext == "iskra"));
    let mut window = eframe::egui::ViewportBuilder::default().with_title("Iskra");
    // The start window is the app's window, as large as it and not resizable, centred; a
    // project's opens as large as it was when the app was last closed, and where it was.
    let kept = left.opening_position().filter(|_| file.is_some());
    window = match (&file, kept) {
        (None, _) => window.with_inner_size(left.start_size()).with_resizable(false).with_maximize_button(false),
        (Some(_), kept) => {
            let window = window.with_min_inner_size(iskra_ui::prefs::WINDOW_SMALLEST).with_inner_size(left.opening_size());
            let window = window.with_maximized(left.window_maximized);
            if let Some(corner) = kept { window.with_position(corner) } else { window }
        }
    };
    // The app's own icon, for the title bar, the taskbar, Alt-Tab and the Dock, where egui's
    // would be. It is made from `assets/iskra.svg` by `cargo xtask icon`.
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/iskra.png")) {
        window = window.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport: window,
        centered: kept.is_none(),
        wgpu_options: graphics::options(&left, &log),
        // The menu winit gives a macOS app has a Quit that ends the process at once, with no
        // chance to ask about unsaved work. Without it, Quit is the app's own, and the bar
        // at the top of the screen is made in `menu_bar`.
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            builder.with_default_menu(false);
        })),
        ..Default::default()
    };
    // The machine's fonts are found while the window opens, so the first text laid out does
    // not wait for them (plan §4.19).
    iskra_ui::find_fonts();
    let ran = eframe::run_native(
        "Iskra",
        options,
        Box::new(move |cc| {
            let log = in_app;
            let app = IskraApp::new(Session::fresh()).with_log(log.clone());
            let app = match Preferences::default_path() {
                Some(path) => app.with_preferences(path),
                None => app,
            };
            // The app opens on the Media page, on what the project holds, and not in a shot.
            let app = app.starting_on_media();
            let app = match &file {
                Some(file) => {
                    let mut app = app.with_file(file);
                    app.placed_where_left();
                    app
                }
                None => app.with_start_window(),
            };
            // Frames for playback are made on the card the window is drawn with.
            let app = match &cc.wgpu_render_state {
                Some(state) => {
                    let card = state.adapter.get_info();
                    log.say(format!("graphics: {} ({:?}, {:?}), driver {} {}", card.name, card.backend, card.device_type, card.driver, card.driver_info));
                    let mut found: Vec<String> = Vec::new();
                    for name in state.available_adapters.iter().map(|adapter| adapter.get_info().name) {
                        if !found.contains(&name) {
                            found.push(name);
                        }
                    }
                    let app = app.with_cards(found, format!("{} through {}", card.name, card.backend));
                    app.with_gpu(state.device.clone(), state.queue.clone(), state.renderer.clone())
                }
                None => {
                    log.say("graphics: the window has no graphics card to draw with");
                    app
                }
            };
            #[cfg(target_os = "linux")]
            tablet::spawn_x11(&app.stylus.feed, cc, &log);
            #[cfg(windows)]
            for line in iskra_tablet_windows::report() {
                log.say(format!("pen: {line}"));
            }
            // The window the tablet driver's WinTab is read on; how, the preferences say, from
            // the first frame on (see `follow_tablet_input`).
            #[cfg(windows)]
            let hwnd = {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                cc.window_handle().ok().and_then(|handle| match handle.as_raw() {
                    RawWindowHandle::Win32(window) => Some(window.hwnd.get()),
                    _ => None,
                })
            };
            // Without a sound output the app plays silently, by its own clock.
            let sound = sound_out::open(app.player(), &log);
            log.say(if sound.is_some() { "sound output: open" } else { "sound output: none could be opened, so the app plays silently" });
            let app = if sound.is_some() { app.with_sound() } else { app };
            // On macOS the menus are in the system's bar at the top of the screen, and the
            // window has none of its own; unless the bar could not be made.
            #[cfg(target_os = "macos")]
            let (app, menu) = {
                let mut app = app;
                let menu = match menu_bar::install(&cc.egui_ctx, &app.menu_bar()) {
                    Ok(menu) => Some(menu),
                    Err(why) => {
                        log.say(format!("the menu bar could not be made: {why}"));
                        None
                    }
                };
                app.set_system_menus(menu.is_some());
                (app, menu)
            };
            Ok(Box::new(Shell {
                _sound: sound,
                input: None,
                #[cfg(target_os = "macos")]
                _tablet: tablet::install_macos(&app.stylus.feed, &log),
                #[cfg(target_os = "macos")]
                menu,
                #[cfg(windows)]
                wintab: None,
                #[cfg(windows)]
                hwnd,
                #[cfg(windows)]
                tablet_input: None,
                log,
                app,
            }))
        }),
    );
    if let Err(why) = &ran {
        log.say(format!("the window could not be opened: {why}"));
    }
    ran
}
