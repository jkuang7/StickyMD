use anyhow::Context;
use tauri::menu::{
    Menu, MenuBuilder, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu, SubmenuBuilder,
};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_log::log;

use crate::save_load::save_settings;
use crate::settings::MenuSettings;
use crate::windows::{
    arrange_notes_on_this_side_below_focused, change_focused_note_font_size, create_sticky,
    cycle_focus, request_close_sticky, reset_note_positions, restore_all_notes,
    restore_last_closed, set_color, show_version_window, snap_window, toggle_note_visibility,
    toggle_shortcuts_window, Direction,
};

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, Copy)]
pub enum MenuCommand {
    NewNote,
    CloseNote,
    ReopenClosedNote,
    RestoreAllNotes,
    ResetPositions,
    ArrangeNotesOnThisSideBelowCurrent,
    ToggleNoteVisibility,
    NextNote,
    PrevNote,
    IncreaseFontSize,
    DecreaseFontSize,
    Color(u8),
    Snap(Direction),
    PartialSnap(Direction),
    BringToFront,
    AutoStart,
    ToggleShortcuts,
    ShowVersion,
}

impl From<MenuCommand> for MenuId {
    fn from(command: MenuCommand) -> Self {
        MenuId(serde_json::to_string(&command).expect("Could not serialize MenuCommand enum"))
    }
}

impl TryFrom<MenuId> for MenuCommand {
    type Error = anyhow::Error;
    fn try_from(value: MenuId) -> Result<Self, Self::Error> {
        serde_json::from_str(&value.0).context(format!(
            "Could not deserialize {:?} into MenuCommand",
            value
        ))
    }
}

fn create_window_submenu(app: &AppHandle) -> Result<Submenu<Wry>, anyhow::Error> {
    let settings = app.state::<MenuSettings>();

    let menu = SubmenuBuilder::new(app, "About")
        .items(&[
            &PredefinedMenuItem::quit(app, None)?,
            &MenuItem::with_id(
                app,
                MenuCommand::CloseNote,
                "Close Note",
                true,
                Some("Cmd+W"),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::ReopenClosedNote,
                "Reopen Last Closed Note",
                true,
                Some("Cmd+Shift+T"),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::RestoreAllNotes,
                "Restore All Notes",
                true,
                Some("Cmd+Shift+U"),
            )?,
            &MenuItem::with_id(app, MenuCommand::NewNote, "New Note", true, Some("Cmd+N"))?,
            &MenuItem::with_id(
                app,
                MenuCommand::ToggleNoteVisibility,
                "Hide/Show All Notes",
                true,
                Some("Cmd+Shift+H"),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::ResetPositions,
                "Reset Note Positions",
                true,
                Some("Cmd+Shift+R"),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::ArrangeNotesOnThisSideBelowCurrent,
                "Arrange Notes on This Side Below Current Note",
                true,
                Some("Cmd+Shift+L"),
            )?,
        ])
        .separator()
        .items(&[
            &MenuItem::with_id(
                app,
                MenuCommand::NextNote,
                "Focus Next Note",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::PrevNote,
                "Focus Previous Note",
                true,
                None::<&str>,
            )?,
        ])
        .separator()
        .items(&[&settings.bring_to_front, &settings.autostart])
        .build()?;

    Ok(menu)
}

fn create_snap_submenu(app: &AppHandle) -> Result<Submenu<Wry>, anyhow::Error> {
    let menu = SubmenuBuilder::new(app, "Snap")
        .items(&[
            &MenuItem::with_id(
                app,
                MenuCommand::Snap(Direction::Up),
                "Up",
                true,
                Some("Cmd+Alt+Up"),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::Snap(Direction::Down),
                "Down",
                true,
                Some("Cmd+Alt+Down"),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::Snap(Direction::Left),
                "Left",
                true,
                Some("Cmd+Alt+Left"),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::Snap(Direction::Right),
                "Right",
                true,
                Some("Cmd+Alt+Right"),
            )?,
        ])
        .build()?;

    Ok(menu)
}

fn create_partial_snap_submenu(app: &AppHandle) -> Result<Submenu<Wry>, anyhow::Error> {
    let menu = SubmenuBuilder::new(app, "Partial Snap")
        .items(&[
            &MenuItem::with_id(
                app,
                MenuCommand::PartialSnap(Direction::Up),
                "Up",
                true,
                Some("Cmd+Alt+Shift+Up"),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::PartialSnap(Direction::Down),
                "Down",
                true,
                Some("Cmd+Alt+Shift+Down"),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::PartialSnap(Direction::Left),
                "Left",
                true,
                Some("Cmd+Alt+Shift+Left"),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::PartialSnap(Direction::Right),
                "Right",
                true,
                Some("Cmd+Alt+Shift+Right"),
            )?,
        ])
        .build()?;

    Ok(menu)
}

fn create_edit_submenu(app: &AppHandle) -> Result<Submenu<Wry>, anyhow::Error> {
    let menu = SubmenuBuilder::new(app, "Edit")
        .items(&[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
        ])
        .separator()
        .items(&[
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
        ])
        .build()?;

    Ok(menu)
}

fn create_view_submenu(app: &AppHandle) -> Result<Submenu<Wry>, anyhow::Error> {
    SubmenuBuilder::new(app, "View")
        .items(&[
            &MenuItem::with_id(
                app,
                MenuCommand::IncreaseFontSize,
                "Increase Font Size",
                true,
                Some("Cmd+="),
            )?,
            &MenuItem::with_id(
                app,
                MenuCommand::DecreaseFontSize,
                "Decrease Font Size",
                true,
                Some("Cmd+-"),
            )?,
        ])
        .build()
        .map_err(Into::into)
}

fn create_color_menu(app: &AppHandle) -> Result<Submenu<Wry>, anyhow::Error> {
    let menu = SubmenuBuilder::new(app, "Color")
        .items(&[
            &MenuItem::with_id(app, MenuCommand::Color(0), "Color 1", true, Some("Cmd+1"))?,
            &MenuItem::with_id(app, MenuCommand::Color(1), "Color 2", true, Some("Cmd+2"))?,
            &MenuItem::with_id(app, MenuCommand::Color(2), "Color 3", true, Some("Cmd+3"))?,
            &MenuItem::with_id(app, MenuCommand::Color(3), "Color 4", true, Some("Cmd+4"))?,
            &MenuItem::with_id(app, MenuCommand::Color(4), "Color 5", true, Some("Cmd+5"))?,
            &MenuItem::with_id(app, MenuCommand::Color(5), "Color 6", true, Some("Cmd+6"))?,
            &MenuItem::with_id(app, MenuCommand::Color(6), "Color 7", true, Some("Cmd+7"))?,
        ])
        .build()?;

    Ok(menu)
}

fn create_help_menu(app: &AppHandle) -> Result<Submenu<Wry>, anyhow::Error> {
    SubmenuBuilder::new(app, "Help")
        .items(&[
            &MenuItem::with_id(
                app,
                MenuCommand::ToggleShortcuts,
                "Keyboard Shortcuts…",
                true,
                Some("Cmd+/"),
            )?,
            &MenuItem::with_id(app, MenuCommand::ShowVersion, "Update", true, None::<&str>)?,
        ])
        .build()
        .map_err(Into::into)
}

pub fn create_menu(app: &AppHandle) -> Result<Menu<Wry>, anyhow::Error> {
    let menu = MenuBuilder::new(app)
        .items(&[
            &create_window_submenu(app)?,
            &create_edit_submenu(app)?,
            &create_view_submenu(app)?,
            &create_snap_submenu(app)?,
            &create_partial_snap_submenu(app)?,
            &create_color_menu(app)?,
            &create_help_menu(app)?,
        ])
        .build()?;

    Ok(menu)
}

pub fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    match MenuCommand::try_from(event.id) {
        Ok(command) => {
            if let Err(e) = match command {
                MenuCommand::NewNote => create_sticky(app).map(|_| ()),
                MenuCommand::ResetPositions => reset_note_positions(app),
                MenuCommand::ArrangeNotesOnThisSideBelowCurrent => {
                    arrange_notes_on_this_side_below_focused(app)
                }
                MenuCommand::Snap(direction) => snap_window(app, direction, false),
                MenuCommand::PartialSnap(direction) => snap_window(app, direction, true),
                MenuCommand::CloseNote => request_close_sticky(app),
                MenuCommand::ReopenClosedNote => restore_last_closed(app),
                MenuCommand::RestoreAllNotes => restore_all_notes(app),
                MenuCommand::ToggleNoteVisibility => toggle_note_visibility(app),
                MenuCommand::NextNote => cycle_focus(app, false),
                MenuCommand::PrevNote => cycle_focus(app, true),
                MenuCommand::IncreaseFontSize => change_focused_note_font_size(app, true),
                MenuCommand::DecreaseFontSize => change_focused_note_font_size(app, false),
                MenuCommand::Color(index) => set_color(app, index),
                MenuCommand::BringToFront => save_settings(app),
                MenuCommand::AutoStart => apply_autostart_preference(app),
                MenuCommand::ToggleShortcuts => toggle_shortcuts_window(app),
                MenuCommand::ShowVersion => show_version_window(app),
                // _ => Err(anyhow::anyhow!("unimplemented command: {:?}", command)),
            } {
                log::error!("Error executing command: {:?} : {:#}", command, e);
            };
        }
        Err(e) => {
            log::warn!("{:#}", e)
        }
    };
}

fn apply_autostart_preference(app: &AppHandle) -> anyhow::Result<()> {
    let settings = app.state::<MenuSettings>();
    let desired = settings.autostart()?;
    let previous = !desired;

    if cfg!(debug_assertions) {
        settings.autostart.set_checked(false)?;
        anyhow::bail!("Development builds cannot be added to login items");
    }

    let manager = app.autolaunch();
    let apply = if desired {
        manager.enable()
    } else {
        manager.disable()
    };

    if let Err(error) = apply {
        settings.autostart.set_checked(previous)?;
        return Err(error.into());
    }

    if let Err(error) = save_settings(app) {
        let rollback = if previous {
            manager.enable()
        } else {
            manager.disable()
        };
        settings.autostart.set_checked(previous)?;
        if let Err(rollback_error) = rollback {
            log::error!("Could not roll back login-item state: {rollback_error}");
        }
        return Err(error);
    }

    Ok(())
}
