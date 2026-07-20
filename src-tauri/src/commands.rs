use std::{collections::HashSet, iter::once, sync::Mutex};

use serde_json::Value;
use tauri::Manager;

use crate::{
    save_load::{note_id_from_label, NoteRepository},
    settings::MenuSettings,
    windows::{
        apply_note_pin_state, arrange_notes_on_this_side_below, change_note_font_size,
        close_window_and_archive, create_sticky, set_window_collapsed, sorted_windows,
        GeometryIndex,
    },
};

#[derive(Default)]
enum QuitState {
    #[default]
    Idle,
    Waiting(HashSet<String>),
    Ready,
}

#[derive(Default)]
pub struct QuitCoordinator(Mutex<QuitState>);

impl QuitCoordinator {
    pub fn begin(&self, labels: HashSet<String>) -> anyhow::Result<bool> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Quit coordinator lock poisoned"))?;
        if !matches!(*state, QuitState::Idle) {
            return Ok(false);
        }
        *state = if labels.is_empty() {
            QuitState::Ready
        } else {
            QuitState::Waiting(labels)
        };
        Ok(true)
    }

    fn acknowledge(&self, label: &str) -> anyhow::Result<bool> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Quit coordinator lock poisoned"))?;
        let QuitState::Waiting(labels) = &mut *state else {
            return Ok(false);
        };
        labels.remove(label);
        if labels.is_empty() {
            *state = QuitState::Ready;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn is_ready(&self) -> anyhow::Result<bool> {
        let state = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Quit coordinator lock poisoned"))?;
        Ok(matches!(*state, QuitState::Ready))
    }
}

#[tauri::command]
pub fn bring_all_to_front(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    settings: tauri::State<MenuSettings>,
) -> Result<(), String> {
    if !settings
        .bring_to_front()
        .map_err(|error| error.to_string())?
    {
        return Ok(());
    }

    sorted_windows(&app)
        .into_iter()
        .chain(once(window))
        .for_each(|window| {
            #[cfg(target_os = "macos")]
            {
                use objc2_app_kit::NSWindow;

                if let Ok(ns_window_ptr) = window.ns_window() {
                    unsafe {
                        let ns_window = &mut *(ns_window_ptr as *mut NSWindow);
                        ns_window.orderFrontRegardless();
                    }
                }
            }
        });
    Ok(())
}

#[tauri::command]
pub fn start_window_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSWindow};

        let Some(main_thread) = MainThreadMarker::new() else {
            tauri_plugin_log::log::error!(
                "Window drag command did not run on the macOS main thread"
            );
            return Err("Window drag must start on the macOS main thread".to_string());
        };
        let ns_window_ptr = window.ns_window().map_err(|error| error.to_string())?;
        let ns_window = unsafe { &*(ns_window_ptr as *const NSWindow) };
        let current_event = NSApplication::sharedApplication(main_thread)
            .currentEvent()
            .filter(|event| {
                event.r#type() == NSEventType::LeftMouseDown
                    && event.windowNumber() == ns_window.windowNumber()
            });
        let event = if let Some(event) = current_event {
            event
        } else {
            let location = ns_window.convertPointFromScreen(NSEvent::mouseLocation());
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                NSEventType::LeftMouseDown,
                location,
                NSEventModifierFlags::empty(),
                0.0,
                ns_window.windowNumber(),
                None,
                0,
                1,
                1.0,
            )
            .ok_or_else(|| "Could not construct the macOS window drag event".to_string())?
        };

        ns_window.performWindowDragWithEvent(&event);
        window.set_focus().map_err(|error| error.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        window.start_dragging().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())
    }
}

#[tauri::command]
pub fn arrange_notes_on_this_side_below_current_note(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    arrange_notes_on_this_side_below(&app, &window).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn change_font_size(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    increase: bool,
) -> Result<(), String> {
    change_note_font_size(&app, &window, increase).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn create_note(app: tauri::AppHandle) -> Result<(), String> {
    create_sticky(&app)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn close_window(window: tauri::WebviewWindow) -> Result<(), String> {
    close_window_and_archive(&window).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_note(
    window: tauri::WebviewWindow,
    document: Value,
    color: String,
) -> Result<(), String> {
    if document.get("type").and_then(Value::as_str) != Some("doc") {
        return Err("Refusing to save a document whose root type is not 'doc'".into());
    }

    let id = note_id_from_label(window.label()).map_err(|error| error.to_string())?;
    let geometry = window
        .state::<GeometryIndex>()
        .get(id)
        .map_err(|error| error.to_string())?;
    let scale_factor = window.scale_factor().map_err(|error| error.to_string())?;
    let position = geometry.position.to_logical::<i32>(scale_factor);
    let size = geometry.size.to_logical::<u32>(scale_factor);
    let pinned = window
        .is_always_on_top()
        .map_err(|error| error.to_string())?;
    let repository = window.state::<NoteRepository>();

    repository
        .update(id, |note| {
            note.document = document;
            note.color = color;
            note.x = position.x;
            note.y = position.y;
            note.pinned = pinned;
            if !note.collapsed {
                note.expanded_width = size.width.max(150);
                note.expanded_height = size.height.max(80);
            }
            Ok(())
        })
        .map_err(|error| error.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn save_geometry(window: tauri::WebviewWindow) -> Result<(), String> {
    let id = note_id_from_label(window.label()).map_err(|error| error.to_string())?;
    let geometry = window
        .state::<GeometryIndex>()
        .get(id)
        .map_err(|error| error.to_string())?;
    let scale_factor = window.scale_factor().map_err(|error| error.to_string())?;
    let position = geometry.position.to_logical::<i32>(scale_factor);
    let size = geometry.size.to_logical::<u32>(scale_factor);
    window
        .state::<NoteRepository>()
        .update_geometry_if_changed(id, position.x, position.y, size.width, size.height)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_note_always_on_top(
    window: tauri::WebviewWindow,
    always_on_top: bool,
) -> Result<(), String> {
    apply_note_pin_state(&window, always_on_top).map_err(|error| error.to_string())?;
    let id = note_id_from_label(window.label()).map_err(|error| error.to_string())?;
    window
        .state::<NoteRepository>()
        .update(id, |note| {
            note.pinned = always_on_top;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_collapsed(window: tauri::WebviewWindow, collapsed: bool) -> Result<(), String> {
    set_window_collapsed(&window, collapsed).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn acknowledge_quit(
    window: tauri::WebviewWindow,
    coordinator: tauri::State<QuitCoordinator>,
) -> Result<(), String> {
    if coordinator
        .acknowledge(window.label())
        .map_err(|error| error.to_string())?
    {
        window.app_handle().exit(0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_is_ready_only_after_every_window_acknowledges() {
        let coordinator = QuitCoordinator::default();
        assert!(!coordinator.is_ready().unwrap());
        assert!(coordinator
            .begin(HashSet::from(["one".into(), "two".into()]))
            .unwrap());
        assert!(!coordinator.acknowledge("one").unwrap());
        assert!(!coordinator.is_ready().unwrap());
        assert!(coordinator.acknowledge("two").unwrap());
        assert!(coordinator.is_ready().unwrap());
    }

    #[test]
    fn quit_without_windows_is_ready_immediately() {
        let coordinator = QuitCoordinator::default();
        assert!(coordinator.begin(HashSet::new()).unwrap());
        assert!(coordinator.is_ready().unwrap());
    }
}
