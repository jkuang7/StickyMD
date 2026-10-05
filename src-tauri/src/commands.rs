use std::{collections::HashSet, iter::once, sync::Mutex};

use serde_json::Value;
use tauri::Manager;
use tauri_plugin_log::log;

use crate::{
    groups::{
        close_window as close_surface_window, link_windows_on_this_side_below,
        resize_note_height as resize_native_note_height, run_window_drag, set_window_collapsed,
        settle_window_geometry, GroupRuntime,
    },
    pinned_windows::sync_pinned_window_registry,
    save_load::{note_id_from_label, NoteRepository},
    settings::MenuSettings,
    user_action_workflow::{PinTarget, PinWorkflow},
    windows::{
        apply_window_pin_state, change_note_font_size, create_sticky,
        snap_note_window as snap_target_note_window, sorted_windows, Direction,
    },
};

const LEFT_MOUSE_BUTTON_MASK: usize = 1;

fn left_mouse_button_is_pressed_in(mask: usize) -> bool {
    mask & LEFT_MOUSE_BUTTON_MASK != 0
}

#[cfg(target_os = "macos")]
fn left_mouse_button_is_pressed() -> bool {
    use objc2_app_kit::NSEvent;

    left_mouse_button_is_pressed_in(NSEvent::pressedMouseButtons())
}

#[cfg(not(target_os = "macos"))]
fn left_mouse_button_is_pressed() -> bool {
    false
}

#[derive(Default)]
enum QuitState {
    #[default]
    Idle,
    Waiting {
        attempt: u64,
        pending: HashSet<String>,
    },
    Ready,
}

#[derive(Default)]
struct QuitProgress {
    state: QuitState,
    last_attempt: u64,
}

#[derive(Default)]
pub struct QuitCoordinator(Mutex<QuitProgress>);

#[cfg(target_os = "macos")]
pub fn coordinate_native_quit(app: &tauri::AppHandle) -> anyhow::Result<()> {
    use objc2::{
        ffi::class_addMethod,
        msg_send,
        runtime::{AnyClass, AnyObject, Imp, Sel},
        sel, MainThreadMarker,
    };
    use objc2_app_kit::NSApplication;
    use std::sync::OnceLock;

    static QUIT_APP: OnceLock<tauri::AppHandle> = OnceLock::new();

    unsafe extern "C-unwind" fn request_coordinated_quit(
        _: &AnyObject,
        _: Sel,
        _: &AnyObject,
    ) -> usize {
        if let Some(app) = QUIT_APP.get() {
            app.exit(0);
        }
        0
    }

    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| anyhow::anyhow!("Native quit must be configured on the main thread"))?;
    let application = NSApplication::sharedApplication(main_thread);
    let delegate: *mut AnyObject = unsafe { msg_send![&*application, delegate] };
    let delegate = unsafe { delegate.as_ref() }
        .ok_or_else(|| anyhow::anyhow!("Native application delegate is unavailable"))?;
    QUIT_APP
        .set(app.clone())
        .map_err(|_| anyhow::anyhow!("Native quit is already configured"))?;
    let callback: unsafe extern "C-unwind" fn(&AnyObject, Sel, &AnyObject) -> usize =
        request_coordinated_quit;
    let implementation: Imp = unsafe { std::mem::transmute(callback) };
    let installed = unsafe {
        class_addMethod(
            delegate.class() as *const AnyClass as *mut AnyClass,
            sel!(applicationShouldTerminate:),
            implementation,
            c"Q@:@".as_ptr(),
        )
    };
    anyhow::ensure!(
        installed.as_bool(),
        "Could not coordinate native application quit"
    );
    Ok(())
}

impl QuitCoordinator {
    pub fn begin(&self, labels: HashSet<String>) -> anyhow::Result<Option<u64>> {
        let mut progress = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Quit coordinator lock poisoned"))?;
        if !matches!(progress.state, QuitState::Idle) {
            return Ok(None);
        }
        progress.last_attempt += 1;
        let attempt = progress.last_attempt;
        progress.state = if labels.is_empty() {
            QuitState::Ready
        } else {
            QuitState::Waiting {
                attempt,
                pending: labels,
            }
        };
        Ok(Some(attempt))
    }

    fn acknowledge(&self, label: &str, attempt: u64) -> anyhow::Result<bool> {
        self.remove_pending(label, Some(attempt))
    }

    pub fn destroyed(&self, label: &str) -> anyhow::Result<bool> {
        self.remove_pending(label, None)
    }

    fn remove_pending(&self, label: &str, expected_attempt: Option<u64>) -> anyhow::Result<bool> {
        let mut progress = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Quit coordinator lock poisoned"))?;
        let QuitState::Waiting { attempt, pending } = &mut progress.state else {
            return Ok(false);
        };
        if expected_attempt.is_some_and(|expected| expected != *attempt) {
            return Ok(false);
        }
        pending.remove(label);
        if pending.is_empty() {
            progress.state = QuitState::Ready;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn fail(&self, attempt: u64, notify: impl FnOnce(u64)) -> anyhow::Result<()> {
        let mut progress = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Quit coordinator lock poisoned"))?;
        if !matches!(progress.state, QuitState::Waiting { attempt: current, .. } if current == attempt)
        {
            return Ok(());
        }
        progress.state = QuitState::Idle;
        drop(progress);
        notify(attempt);
        Ok(())
    }

    pub fn is_ready(&self) -> anyhow::Result<bool> {
        let progress = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Quit coordinator lock poisoned"))?;
        Ok(matches!(progress.state, QuitState::Ready))
    }
}

pub fn stop_quit(app: &tauri::AppHandle, attempt: u64, error: &str) -> anyhow::Result<()> {
    use tauri::Emitter;
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

    app.state::<QuitCoordinator>().fail(attempt, |attempt| {
        if let Err(error) = app.emit("quit_save_failed", attempt) {
            log::error!("Could not notify notes that quit stopped: {error}");
        }
        app.dialog()
            .message(format!(
                "A note could not be saved and quit stopped.\n{error}"
            ))
            .title("Sticky action failed")
            .kind(MessageDialogKind::Error)
            .show(|_| {});
    })
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
    run_window_drag(&window, || {
        #[cfg(target_os = "macos")]
        {
            use objc2::MainThreadMarker;
            use objc2_app_kit::{
                NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSWindow,
            };

            let Some(main_thread) = MainThreadMarker::new() else {
                tauri_plugin_log::log::error!(
                    "Window drag command did not run on the macOS main thread"
                );
                return Err(anyhow::anyhow!(
                    "Window drag must start on the macOS main thread"
                ));
            };
            let ns_window_ptr = window.ns_window()?;
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
                .ok_or_else(|| anyhow::anyhow!("Could not construct the macOS window drag event"))?
            };

            ns_window.performWindowDragWithEvent(&event);
            window.set_focus()?;
        }

        #[cfg(not(target_os = "macos"))]
        {
            window.start_dragging()?;
            window.set_focus()?;
        }
        Ok(())
    })
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn link_windows_on_this_side_below_current_window(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    link_windows_on_this_side_below(&app, &window).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn resize_note_height(window: tauri::WebviewWindow, height: u32) -> Result<(), String> {
    resize_native_note_height(&window, height).map_err(|error| error.to_string())
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
pub fn snap_window(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    direction: Direction,
    partial: bool,
) -> Result<(), String> {
    snap_target_note_window(&app, &window, direction, partial).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn create_note(app: tauri::AppHandle) -> Result<(), String> {
    create_sticky(&app)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn close_window(window: tauri::WebviewWindow) -> Result<(), String> {
    close_surface_window(&window).map_err(|error| error.to_string())
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

    let group_runtime = window.state::<GroupRuntime>();
    let _operation = group_runtime.lock().map_err(|error| error.to_string())?;
    let id = note_id_from_label(window.label()).map_err(|error| error.to_string())?;
    let repository = window.state::<NoteRepository>();

    repository
        .update(id, |note| {
            note.document = document;
            note.color = color;
            Ok(())
        })
        .map_err(|error| error.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn save_geometry(window: tauri::WebviewWindow) -> Result<bool, String> {
    if left_mouse_button_is_pressed() {
        return Ok(false);
    }
    settle_window_geometry(&window)
        .map(|()| true)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_note_always_on_top(
    window: tauri::WebviewWindow,
    always_on_top: bool,
) -> Result<(), String> {
    let id = note_id_from_label(window.label())
        .map_err(|error| error.to_string())?
        .to_owned();
    PinWorkflow::perform(&mut NotePinTarget { window, id }, always_on_top)
        .map_err(|error| error.to_string())
}

struct NotePinTarget {
    window: tauri::WebviewWindow,
    id: String,
}

impl PinTarget for NotePinTarget {
    fn current_pinned(&self) -> anyhow::Result<bool> {
        Ok(self.window.state::<NoteRepository>().get(&self.id)?.pinned)
    }

    fn set_native_pinned(&mut self, pinned: bool) -> anyhow::Result<()> {
        apply_window_pin_state(&self.window, pinned)
    }

    fn set_durable_pinned(&mut self, pinned: bool) -> anyhow::Result<()> {
        self.window
            .state::<NoteRepository>()
            .update(&self.id, |note| {
                note.pinned = pinned;
                Ok(())
            })
            .map(|_| ())
    }

    fn sync_pinned_registry(&mut self) -> anyhow::Result<()> {
        sync_pinned_window_registry(self.window.app_handle(), None)
    }

    fn focus(&mut self) -> anyhow::Result<()> {
        self.window.set_focus().map_err(Into::into)
    }
}

#[tauri::command]
pub fn set_collapsed(window: tauri::WebviewWindow, collapsed: bool) -> Result<(), String> {
    set_window_collapsed(&window, collapsed).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn acknowledge_quit(
    attempt: u64,
    window: tauri::WebviewWindow,
    coordinator: tauri::State<QuitCoordinator>,
) -> Result<(), String> {
    if coordinator
        .acknowledge(window.label(), attempt)
        .map_err(|error| error.to_string())?
    {
        window.app_handle().exit(0);
    }
    Ok(())
}

#[tauri::command]
pub fn fail_quit(app: tauri::AppHandle, attempt: u64, error: String) -> Result<(), String> {
    stop_quit(&app, attempt, &error).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_is_ready_only_after_every_window_acknowledges() {
        let coordinator = QuitCoordinator::default();
        assert!(!coordinator.is_ready().unwrap());
        let attempt = coordinator
            .begin(HashSet::from(["one".into(), "two".into()]))
            .unwrap()
            .unwrap();
        assert!(!coordinator.acknowledge("one", attempt).unwrap());
        assert!(!coordinator.is_ready().unwrap());
        assert!(coordinator.acknowledge("two", attempt).unwrap());
        assert!(coordinator.is_ready().unwrap());
    }

    #[test]
    fn quit_without_windows_is_ready_immediately() {
        let coordinator = QuitCoordinator::default();
        assert!(coordinator.begin(HashSet::new()).unwrap().is_some());
        assert!(coordinator.is_ready().unwrap());
    }

    #[test]
    fn failed_quit_notifies_all_notes_once_and_retries_every_note() {
        let coordinator = QuitCoordinator::default();
        let labels = HashSet::from(["one".into(), "two".into()]);
        let first = coordinator.begin(labels.clone()).unwrap().unwrap();
        assert!(coordinator.begin(labels.clone()).unwrap().is_none());
        assert!(!coordinator.acknowledge("one", first).unwrap());
        let mut notices = Vec::new();
        coordinator
            .fail(first, |attempt| {
                for label in &labels {
                    notices.push((label.clone(), attempt));
                }
            })
            .unwrap();
        coordinator
            .fail(first, |_| panic!("Duplicate failure notice"))
            .unwrap();
        assert_eq!(notices.len(), 2);
        assert!(notices.iter().all(|(_, attempt)| *attempt == first));
        assert!(!coordinator.is_ready().unwrap());
        let second = coordinator.begin(labels).unwrap().unwrap();
        assert!(second > first);
        assert!(!coordinator.acknowledge("two", second).unwrap());
        assert!(!coordinator.is_ready().unwrap());
        assert!(coordinator.acknowledge("one", second).unwrap());
    }

    #[test]
    fn stale_quit_confirmations_and_failures_do_not_end_the_retry() {
        let coordinator = QuitCoordinator::default();
        let labels = HashSet::from(["one".into(), "two".into()]);
        let first = coordinator.begin(labels.clone()).unwrap().unwrap();
        coordinator.fail(first, |_| {}).unwrap();
        let second = coordinator.begin(labels).unwrap().unwrap();
        assert!(!coordinator.acknowledge("one", first).unwrap());
        coordinator
            .fail(first, |_| panic!("Stale failure notice"))
            .unwrap();
        assert!(!coordinator.acknowledge("two", second).unwrap());
        assert!(!coordinator.is_ready().unwrap());
        assert!(coordinator.acknowledge("one", second).unwrap());
    }

    #[test]
    fn destroying_the_last_unconfirmed_note_finishes_quit() {
        let coordinator = QuitCoordinator::default();
        let attempt = coordinator
            .begin(HashSet::from(["one".into(), "two".into(), "three".into()]))
            .unwrap()
            .unwrap();
        assert!(!coordinator.destroyed("unknown").unwrap());
        assert!(!coordinator.acknowledge("one", attempt).unwrap());
        assert!(!coordinator.destroyed("one").unwrap());
        assert!(!coordinator.destroyed("two").unwrap());
        assert!(!coordinator.is_ready().unwrap());
        assert!(coordinator.destroyed("three").unwrap());
        assert!(coordinator.is_ready().unwrap());
    }

    #[test]
    fn geometry_settlement_waits_only_for_the_left_mouse_button() {
        assert!(left_mouse_button_is_pressed_in(0b0001));
        assert!(left_mouse_button_is_pressed_in(0b0101));
        assert!(!left_mouse_button_is_pressed_in(0));
        assert!(!left_mouse_button_is_pressed_in(0b0010));
    }
}
