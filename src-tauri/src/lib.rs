use std::collections::HashSet;

use tauri::{App, Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_log::log::{self, LevelFilter};

use crate::commands::*;
use crate::groups::{restore_group_layouts, GroupRuntime};
use crate::menu::{create_menu, handle_menu_event};
use crate::save_load::{load_settings, load_stickies, NoteRepository};
use crate::settings::MenuSettings;
use crate::timers::{
    initialize_alarm_sounds, restore_timer_windows, start_timer_worker, TimerRepository,
    TimerRuntime,
};
use crate::updater::{check_for_update, launch_update};
use crate::windows::{focus_existing_or_create, GeometryIndex, NoteVisibility};

mod commands;
mod groups;
mod menu;
mod pinned_windows;
mod save_load;
mod settings;
mod timers;
mod updater;
mod windows;

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let repository = NoteRepository::load(app.handle())?;
    app.manage(repository);

    let (timer_repository, timers_available) = match TimerRepository::load(app.handle()) {
        Ok(repository) => (repository, true),
        Err(error) => {
            log::error!("Timer storage is unavailable; timers will be skipped: {error:#}");
            (TimerRepository::unavailable(error), false)
        }
    };
    app.manage(timer_repository);

    let menu_settings = load_settings(app.handle())?;
    reconcile_autostart_on_launch(app.handle(), &menu_settings);

    app.manage(menu_settings);

    let menu = create_menu(app.handle())?;
    app.set_menu(menu)?;
    app.on_menu_event(handle_menu_event);
    load_stickies(app.handle())?;
    if timers_available {
        if let Err(error) = initialize_alarm_sounds(app.handle()) {
            log::error!("Timer alarm sound is unavailable: {error:#}");
        }
        if let Err(error) = restore_timer_windows(app.handle()) {
            log::error!("Could not restore timer windows: {error:#}");
        }
        let timer_ids = app
            .state::<TimerRepository>()
            .all()?
            .into_iter()
            .map(|timer| timer.id)
            .collect();
        app.state::<NoteRepository>()
            .prune_missing_timers(&timer_ids)?;
        start_timer_worker(app.handle().clone());
    }
    restore_group_layouts(app.handle())?;

    Ok(())
}

fn reconcile_autostart_on_launch(app: &tauri::AppHandle, settings: &MenuSettings) {
    let manager = app.autolaunch();
    let actual = match manager.is_enabled() {
        Ok(actual) => actual,
        Err(error) => {
            log::error!("Could not inspect login-item state: {error}");
            return;
        }
    };

    if cfg!(debug_assertions) {
        let _ = settings.autostart.set_checked(false);
        let _ = settings.autostart.set_enabled(false);
        if actual {
            if let Err(error) = manager.disable() {
                log::error!("Could not remove development build from login items: {error}");
            }
        }
        return;
    }

    let desired = match settings.autostart() {
        Ok(desired) => desired,
        Err(error) => {
            log::error!("Could not read saved login-item preference: {error:#}");
            return;
        }
    };
    let result = if desired == actual {
        Ok(())
    } else if desired {
        manager.enable()
    } else {
        manager.disable()
    };

    if let Err(error) = result {
        let _ = settings.autostart.set_checked(actual);
        log::error!("Could not reconcile login-item preference: {error}");
    }
    log::info!(
        "Login-item preference: desired={desired}, actual={}",
        manager.is_enabled().unwrap_or(actual)
    );
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Err(error) = focus_existing_or_create(app) {
                log::error!("Could not focus the running app: {error:#}");
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(LevelFilter::Info)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            bring_all_to_front,
            start_window_drag,
            link_windows_on_this_side_below_current_window,
            resize_note_height,
            change_font_size,
            create_note,
            timers::timer_pause,
            timers::timer_resume,
            timers::timer_reset,
            timers::timer_set_elapsed,
            timers::timer_apply_settings,
            timers::set_timer_always_on_top,
            save_note,
            save_geometry,
            close_window,
            set_note_always_on_top,
            set_collapsed,
            acknowledge_quit,
            check_for_update,
            launch_update,
        ])
        .manage(QuitCoordinator::default())
        .manage(GeometryIndex::default())
        .manage(GroupRuntime::default())
        .manage(NoteVisibility::default())
        .manage(TimerRuntime::default())
        .setup(setup)
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, code, .. } => {
                let coordinator = app.state::<QuitCoordinator>();
                match coordinator.is_ready() {
                    Ok(true) => log::info!("exit code: {:?}", code),
                    Ok(false) => {
                        api.prevent_exit();
                        let labels: HashSet<_> = app
                            .webview_windows()
                            .into_keys()
                            .filter(|label| label.starts_with("sticky_"))
                            .collect();
                        let has_windows = !labels.is_empty();
                        match coordinator.begin(labels) {
                            Ok(true) if has_windows => {
                                if let Err(error) = app.emit("flush_before_quit", ()) {
                                    log::error!("Could not request final note saves: {error}");
                                }
                            }
                            Ok(true) => app.exit(0),
                            Ok(false) => {}
                            Err(error) => log::error!("Could not coordinate quit: {error:#}"),
                        }
                    }
                    Err(error) => {
                        api.prevent_exit();
                        log::error!("Could not inspect quit state: {error:#}");
                    }
                }
            }
            tauri::RunEvent::Reopen { .. } => {
                if let Err(error) = focus_existing_or_create(app) {
                    log::error!("Could not reopen notes from the Dock: {error:#}");
                }
            }
            _ => {}
        });
}
