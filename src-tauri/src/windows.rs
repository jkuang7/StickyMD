use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use anyhow::{bail, Context};
use tauri::{
    AppHandle, Emitter, EventTarget, LogicalPosition, LogicalSize, Manager, PhysicalPosition,
    PhysicalSize, WebviewWindow, WindowEvent,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_log::log;

use crate::native_user_action::{
    dispatch_native_user_action, FocusedSurface, NativeUserAction, NativeUserActionTransport,
    SurfaceKind, UserActionOutcome, USER_ACTION_REQUEST_EVENT,
};
use crate::pinned_windows::sync_pinned_window_registry;
use crate::save_load::{note_id_from_label, save_settings, NoteRepository, StoredNote};
use crate::settings::{
    clamp_font_size, MenuSettings, FONT_SIZE_STEP, MAX_FONT_SIZE, MIN_FONT_SIZE,
};
use crate::updater::installed_build_sha;

const GAP: i32 = 20;
const COLLAPSED_HEIGHT: u32 = 24;
const DEFAULT_NOTE_HEIGHT: u32 = 250;
const DEFAULT_NOTE_WIDTH: u32 = 300;
const SHORTCUTS_WINDOW_LABEL: &str = "keyboard_shortcuts";
const VERSION_WINDOW_LABEL: &str = "version";

#[derive(Default)]
pub struct NoteVisibility(Mutex<Option<String>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteGeometry {
    pub position: PhysicalPosition<i32>,
    pub size: PhysicalSize<u32>,
}

#[derive(Default)]
pub struct GeometryIndex(Mutex<HashMap<String, NoteGeometry>>);

pub fn apply_window_pin_state(window: &WebviewWindow, pinned: bool) -> anyhow::Result<()> {
    window.set_always_on_top(pinned)?;

    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

        let ns_window_ptr = window.ns_window()?;
        let mut collection_behavior =
            NSWindowCollectionBehavior::IgnoresCycle | NSWindowCollectionBehavior::Transient;
        if pinned {
            collection_behavior |= NSWindowCollectionBehavior::CanJoinAllApplications
                | NSWindowCollectionBehavior::CanJoinAllSpaces;
        }

        unsafe {
            let ns_window = &*(ns_window_ptr as *const NSWindow);
            ns_window.setCollectionBehavior(collection_behavior);
        }
    }

    Ok(())
}

impl GeometryIndex {
    pub(crate) fn insert(&self, id: String, geometry: NoteGeometry) -> anyhow::Result<()> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("Geometry index lock poisoned"))?
            .insert(id, geometry);
        Ok(())
    }

    pub fn get(&self, id: &str) -> anyhow::Result<NoteGeometry> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("Geometry index lock poisoned"))?
            .get(id)
            .copied()
            .with_context(|| format!("No live geometry for window {id}"))
    }

    pub(crate) fn set_position(
        &self,
        id: &str,
        position: PhysicalPosition<i32>,
    ) -> anyhow::Result<()> {
        let mut geometries = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Geometry index lock poisoned"))?;
        let geometry = geometries
            .get_mut(id)
            .with_context(|| format!("No live geometry for window {id}"))?;
        geometry.position = position;
        Ok(())
    }

    pub(crate) fn set_size(&self, id: &str, size: PhysicalSize<u32>) -> anyhow::Result<()> {
        let mut geometries = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Geometry index lock poisoned"))?;
        let geometry = geometries
            .get_mut(id)
            .with_context(|| format!("No live geometry for window {id}"))?;
        geometry.size = size;
        Ok(())
    }

    pub(crate) fn record_window_event(&self, id: &str, event: &WindowEvent) -> anyhow::Result<()> {
        match event {
            WindowEvent::Moved(position) => self.set_position(id, *position),
            WindowEvent::Resized(size) => self.set_size(id, *size),
            WindowEvent::Destroyed => {
                self.0
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Geometry index lock poisoned"))?
                    .remove(id);
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct WindowRect {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

impl WindowRect {
    fn from_physical(position: PhysicalPosition<i32>, size: PhysicalSize<u32>) -> Self {
        Self {
            x: i64::from(position.x),
            y: i64::from(position.y),
            width: i64::from(size.width),
            height: i64::from(size.height),
        }
    }

    fn right(self) -> i64 {
        self.x + self.width
    }

    fn bottom(self) -> i64 {
        self.y + self.height
    }

    fn contains(self, other: Self) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    fn separated_by(self, other: Self, gap: i64) -> bool {
        self.right() + gap <= other.x
            || other.right() + gap <= self.x
            || self.bottom() + gap <= other.y
            || other.bottom() + gap <= self.y
    }

    fn distance_squared_from(self, other: Self) -> i128 {
        let dx = i128::from(2 * self.x + self.width - (2 * other.x + other.width));
        let dy = i128::from(2 * self.y + self.height - (2 * other.y + other.height));
        dx * dx + dy * dy
    }
}

fn nearest_free_position(
    anchor: WindowRect,
    obstacles: &[WindowRect],
    work_area: WindowRect,
    new_size: PhysicalSize<u32>,
) -> Option<PhysicalPosition<i32>> {
    let gap = i64::from(GAP);
    let width = i64::from(new_size.width);
    let height = i64::from(new_size.height);
    let mut positions = vec![
        (anchor.right() + gap, anchor.y),
        (anchor.x, anchor.bottom() + gap),
        (anchor.x - width - gap, anchor.y),
        (anchor.x, anchor.y - height - gap),
    ];
    let mut xs = vec![anchor.x, work_area.x + gap, work_area.right() - width - gap];
    let mut ys = vec![
        anchor.y,
        work_area.y + gap,
        work_area.bottom() - height - gap,
    ];

    for obstacle in obstacles {
        xs.extend([obstacle.x, obstacle.right() + gap, obstacle.x - width - gap]);
        ys.extend([
            obstacle.y,
            obstacle.bottom() + gap,
            obstacle.y - height - gap,
        ]);
    }
    for x in xs {
        for &y in &ys {
            positions.push((x, y));
        }
    }

    let mut seen = HashSet::new();
    let mut candidates: Vec<_> = positions
        .into_iter()
        .enumerate()
        .filter(|(_, position)| seen.insert(*position))
        .filter_map(|(preference, (x, y))| {
            let candidate = WindowRect {
                x,
                y,
                width,
                height,
            };
            if !work_area.contains(candidate)
                || obstacles
                    .iter()
                    .any(|obstacle| !candidate.separated_by(*obstacle, gap))
            {
                return None;
            }
            Some((
                candidate.distance_squared_from(anchor),
                preference,
                candidate,
            ))
        })
        .collect();
    candidates.sort_by_key(|(distance, preference, _)| (*distance, *preference));
    let (_, _, candidate) = candidates.into_iter().next()?;
    Some(PhysicalPosition::new(
        i32::try_from(candidate.x).ok()?,
        i32::try_from(candidate.y).ok()?,
    ))
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

fn get_focused_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.webview_windows()
        .into_iter()
        .find(|(label, window)| {
            label.starts_with("sticky_") && window.is_focused().unwrap_or(false)
        })
        .map(|(_label, window)| window)
}

fn get_position_and_size(
    window: &WebviewWindow,
) -> Result<(PhysicalPosition<i32>, PhysicalSize<u32>), anyhow::Error> {
    let window_position = window.outer_position().context(format!(
        "Could not get position of window: {}",
        window.label()
    ))?;
    let window_size = window
        .outer_size()
        .context(format!("Could not get size of window: {}", window.label()))?;
    Ok((window_position, window_size))
}

fn window_overlap(start_1: i32, len_1: i32, start_2: i32, len_2: i32) -> bool {
    let end_1 = start_1 + len_1;
    let end_2 = start_2 + len_2;

    let overlap_start = std::cmp::max(start_1, start_2);
    let overlap_end = std::cmp::min(end_1, end_2);
    overlap_end - overlap_start > GAP
}

pub fn snap_note_window(
    app: &AppHandle,
    window: &WebviewWindow,
    direction: Direction,
    partial: bool,
) -> Result<(), anyhow::Error> {
    log::debug!("Snapping window {:?}", direction);

    let runtime_state = app.state::<crate::groups::GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    let (window_position, window_size) = get_position_and_size(window)?;

    let primary_monitor = app
        .primary_monitor()
        .context("could not get primary monitor")?
        .context("no primary monitor")?;

    let active_monitor = app
        .cursor_position()
        .map(|p| p.to_logical(primary_monitor.scale_factor()))
        .and_then(|p| app.monitor_from_point(p.x, p.y))
        .context("could not get cursor position")?
        .context("could not get monitor from cursor position")?;

    let current_monitor = window
        .current_monitor()
        .context(format!(
            "could not find monitor for window to be positioned: {}",
            window.label()
        ))?
        .context("window to be positioned is hidden or otherwise has no display")?;

    let others = app
        .webview_windows()
        .into_iter()
        .filter(|(_, other)| other != window)
        .filter_map(|(_, other)| get_position_and_size(&other).ok())
        .collect::<Vec<_>>();
    let target = snap_target(
        window_position,
        window_size,
        direction,
        partial,
        *current_monitor.position(),
        *current_monitor.size(),
        (current_monitor.name() != active_monitor.name()).then_some(*active_monitor.position()),
        &others,
    );
    crate::groups::move_snapped_note(window, target, &mut runtime)
}

fn snap_target(
    window_position: PhysicalPosition<i32>,
    window_size: PhysicalSize<u32>,
    direction: Direction,
    partial: bool,
    monitor_position: PhysicalPosition<i32>,
    monitor_size: PhysicalSize<u32>,
    other_monitor: Option<PhysicalPosition<i32>>,
    others: &[(PhysicalPosition<i32>, PhysicalSize<u32>)],
) -> PhysicalPosition<i32> {
    if let Some(origin) = other_monitor {
        return PhysicalPosition::new(origin.x + GAP, origin.y + GAP);
    }
    let other_windows = others.iter().copied();

    let viable_edges: Box<dyn Iterator<Item = i32>> =
        if partial {
            match direction {
                Direction::Left => Box::new(other_windows.flat_map(|(position, size)| {
                    [position.x + size.width as i32 + GAP, position.x]
                })),
                Direction::Up => Box::new(other_windows.flat_map(|(position, size)| {
                    [position.y + size.height as i32 + GAP, position.y]
                })),
                Direction::Right => Box::new(other_windows.flat_map(|(position, size)| {
                    [
                        (position.x + size.width as i32) - window_size.width as i32,
                        position.x - (window_size.width as i32 + GAP),
                    ]
                })),
                Direction::Down => Box::new(other_windows.flat_map(|(position, size)| {
                    [
                        (position.y + size.height as i32) - window_size.height as i32,
                        position.y - (window_size.height as i32 + GAP),
                    ]
                })),
            }
        } else {
            match direction {
                Direction::Left => Box::new(other_windows.filter_map(|(position, size)| {
                    if window_overlap(
                        position.y,
                        size.height as i32,
                        window_position.y,
                        window_size.height as i32,
                    ) {
                        Some(position.x + size.width as i32 + GAP)
                    } else {
                        None
                    }
                })),
                Direction::Up => Box::new(other_windows.filter_map(|(position, size)| {
                    if window_overlap(
                        position.x,
                        size.width as i32,
                        window_position.x,
                        window_size.width as i32,
                    ) {
                        Some(position.y + size.height as i32 + GAP)
                    } else {
                        None
                    }
                })),
                Direction::Right => Box::new(other_windows.filter_map(|(position, size)| {
                    if window_overlap(
                        position.y,
                        size.height as i32,
                        window_position.y,
                        window_size.height as i32,
                    ) {
                        Some(position.x - (window_size.width as i32 + GAP))
                    } else {
                        None
                    }
                })),
                Direction::Down => Box::new(other_windows.filter_map(|(position, size)| {
                    if window_overlap(
                        position.x,
                        size.width as i32,
                        window_position.x,
                        window_size.width as i32,
                    ) {
                        Some(position.y - (window_size.height as i32 + GAP))
                    } else {
                        None
                    }
                })),
            }
        };

    match direction {
        Direction::Left => PhysicalPosition {
            x: viable_edges
                .filter(|edge| *edge < window_position.x)
                .max()
                .unwrap_or(monitor_position.x + GAP),
            y: window_position.y,
        },
        Direction::Up => PhysicalPosition {
            x: window_position.x,
            y: viable_edges
                .filter(|edge| *edge < window_position.y)
                .max()
                .unwrap_or(monitor_position.y + GAP),
        },
        Direction::Right => PhysicalPosition {
            x: viable_edges
                .filter(|edge| *edge > window_position.x)
                .min()
                .unwrap_or(
                    ((monitor_position.x + monitor_size.width as i32) - window_size.width as i32)
                        - GAP,
                ),
            y: window_position.y,
        },
        Direction::Down => PhysicalPosition {
            x: window_position.x,
            y: viable_edges
                .filter(|edge| *edge > window_position.y)
                .min()
                .unwrap_or(
                    ((monitor_position.y + monitor_size.height as i32) - window_size.height as i32)
                        - GAP,
                ),
        },
    }
}

pub fn create_sticky(app: &AppHandle) -> Result<WebviewWindow, anyhow::Error> {
    let anchor = get_focused_window(app).or_else(|| sorted_windows(app).into_iter().last());
    let position = anchor
        .as_ref()
        .map(|anchor| -> anyhow::Result<_> {
            let monitor = anchor
                .current_monitor()?
                .or(anchor.primary_monitor()?)
                .context("No monitor available for placing a new note")?;
            let scale_factor = monitor.scale_factor();
            let anchor_rect =
                WindowRect::from_physical(anchor.outer_position()?, anchor.outer_size()?);
            let work_area =
                WindowRect::from_physical(monitor.work_area().position, monitor.work_area().size);
            let new_size =
                LogicalSize::new(DEFAULT_NOTE_WIDTH, DEFAULT_NOTE_HEIGHT).to_physical(scale_factor);
            let obstacles: Vec<_> = app
                .webview_windows()
                .into_values()
                .filter(|window| {
                    window
                        .current_monitor()
                        .ok()
                        .flatten()
                        .is_some_and(|candidate| candidate.name() == monitor.name())
                })
                .filter_map(|window| {
                    get_position_and_size(&window)
                        .ok()
                        .map(|(position, size)| WindowRect::from_physical(position, size))
                })
                .collect();
            nearest_free_position(anchor_rect, &obstacles, work_area, new_size)
                .map(|position| position.to_logical::<i32>(scale_factor))
                .context("No non-overlapping space is available on the current monitor")
        })
        .transpose()?;
    let repository = app.state::<NoteRepository>();
    let default_font_size = app.state::<MenuSettings>().default_font_size()?;
    let note = match position {
        Some(position) => {
            repository.create_at_with_font_size(position.x, position.y, default_font_size)?
        }
        None => repository.create_with_font_size(default_font_size)?,
    };
    match open_sticky(app, &note) {
        Ok(window) => Ok(window),
        Err(open_error) => {
            repository.delete(&note.id).with_context(|| {
                format!("Could not roll back failed note creation after: {open_error:#}")
            })?;
            Err(open_error.context("Could not open the newly created note"))
        }
    }
}

pub fn focus_existing_or_create(app: &AppHandle) -> Result<(), anyhow::Error> {
    open_missing_active_notes(app)?;
    let windows = sorted_windows(app);
    if windows.is_empty() {
        create_sticky(app)?;
        return Ok(());
    }

    for window in &windows {
        window.show()?;
        if window.is_minimized()? {
            window.unminimize()?;
        }
    }
    windows[0].set_focus()?;
    Ok(())
}

pub(crate) fn open_missing_active_notes(app: &AppHandle) -> anyhow::Result<()> {
    let windows = app.webview_windows();
    for note in app.state::<NoteRepository>().active()? {
        if !windows.contains_key(&format!("sticky_{}", note.id)) {
            open_sticky(app, &note)?;
        }
    }
    Ok(())
}

pub fn open_sticky(app: &AppHandle, note: &StoredNote) -> Result<WebviewWindow, anyhow::Error> {
    log::debug!("Creating new sticky window");
    let label = format!("sticky_{}", note.id);

    #[derive(serde::Serialize)]
    struct StickyInit<'a> {
        id: &'a str,
        document: &'a serde_json::Value,
        color: &'a str,
        collapsed: bool,
        always_on_top: bool,
        font_size: u8,
    }

    let init = StickyInit {
        id: &note.id,
        document: &note.document,
        color: &note.color,
        collapsed: note.collapsed,
        always_on_top: note.pinned,
        font_size: note.font_size,
    };
    let init_script = format!("window.__STICKY_INIT__ = {}", serde_json::to_string(&init)?);

    let window_height = if note.collapsed {
        COLLAPSED_HEIGHT
    } else {
        note.expanded_height.max(80)
    };
    let builder =
        tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App("index.html".into()))
            .title("Sticky")
            .decorations(false)
            .maximizable(false)
            .resizable(!note.collapsed)
            .visible(true)
            .accept_first_mouse(true)
            .initialization_script(init_script)
            .inner_size(note.expanded_width.max(150) as f64, window_height as f64)
            .position(note.x as f64, note.y as f64)
            .always_on_top(note.pinned)
            .prevent_overflow();

    let window = builder.build().context("Could not create sticky window")?;
    app.state::<GeometryIndex>().insert(
        note.id.clone(),
        NoteGeometry {
            position: window.outer_position()?,
            size: window.outer_size()?,
        },
    )?;
    let app_clone = app.clone();
    let note_id = note.id.clone();
    window.on_window_event(move |event| {
        if let Err(error) = app_clone
            .state::<GeometryIndex>()
            .record_window_event(&note_id, event)
        {
            log::error!("Could not update live geometry for note {note_id}: {error:#}");
        }
        if matches!(event, WindowEvent::CloseRequested { .. }) {
            let _ = cycle_focus(&app_clone, false);
        }
        if matches!(event, WindowEvent::Destroyed) {
            if let Err(error) = sync_pinned_window_registry(&app_clone, Some(&note_id)) {
                log::error!(
                    "Could not remove destroyed note {note_id} from pinned-window registry: {error:#}"
                );
            }
        }
    });

    apply_window_pin_state(&window, note.pinned)?;
    if let Err(error) = sync_pinned_window_registry(app, None) {
        log::error!(
            "Could not register pin state for opened note {}: {error:#}",
            note.id
        );
    }

    Ok(window)
}

struct AppNativeUserActionTransport<'a> {
    app: &'a AppHandle,
}

impl NativeUserActionTransport for AppNativeUserActionTransport<'_> {
    fn resolve_focused(&mut self) -> anyhow::Result<FocusedSurface> {
        for (label, window) in self.app.webview_windows() {
            if window
                .is_focused()
                .with_context(|| format!("Could not inspect focus for {label}"))?
            {
                let kind = if label.starts_with("sticky_") {
                    SurfaceKind::Note
                } else if label.starts_with("timer_") {
                    SurfaceKind::Timer
                } else {
                    SurfaceKind::Utility
                };
                return Ok(FocusedSurface { label, kind });
            }
        }
        anyhow::bail!("No window is currently focused")
    }

    fn emit(&mut self, target: &FocusedSurface, action: &NativeUserAction) -> anyhow::Result<()> {
        let window = self
            .app
            .get_webview_window(&target.label)
            .with_context(|| format!("The resolved surface {} is no longer open", target.label))?;
        window
            .emit_to(
                EventTarget::webview_window(&target.label),
                USER_ACTION_REQUEST_EVENT,
                action,
            )
            .context("Could not deliver the user action")
    }

    fn close_utility(&mut self, target: &FocusedSurface) -> anyhow::Result<()> {
        self.app
            .get_webview_window(&target.label)
            .with_context(|| format!("The resolved window {} is no longer open", target.label))?
            .close()
            .context("Could not close the focused utility window")
    }

    fn render(&mut self, outcome: &UserActionOutcome) {
        let message = match outcome {
            UserActionOutcome::Failed { message } | UserActionOutcome::Busy { message } => message,
            UserActionOutcome::Succeeded | UserActionOutcome::Cancelled => return,
        };
        self.app
            .dialog()
            .message(message)
            .title("Sticky action failed")
            .kind(MessageDialogKind::Error)
            .show(|_| {});
    }
}

fn request_native_user_action(
    app: &AppHandle,
    action: NativeUserAction,
) -> Result<(), anyhow::Error> {
    let mut transport = AppNativeUserActionTransport { app };
    match dispatch_native_user_action(&mut transport, action) {
        UserActionOutcome::Succeeded | UserActionOutcome::Cancelled => Ok(()),
        UserActionOutcome::Busy { message } | UserActionOutcome::Failed { message } => {
            anyhow::bail!(message)
        }
    }
}

pub(crate) fn request_mapped_native_user_action(
    app: &AppHandle,
    action: Result<NativeUserAction, anyhow::Error>,
) -> Result<(), anyhow::Error> {
    match action {
        Ok(action) => request_native_user_action(app, action),
        Err(error) => {
            let mut transport = AppNativeUserActionTransport { app };
            transport.render(&UserActionOutcome::Failed {
                message: format!("{error:#}"),
            });
            Err(error)
        }
    }
}

pub(crate) fn close_ungrouped_window_and_archive(
    window: &WebviewWindow,
) -> Result<(), anyhow::Error> {
    let id = note_id_from_label(window.label())?.to_string();
    let repository = window.state::<NoteRepository>();
    let previous_closed_at = repository.get(&id)?.closed_at;
    repository.close(&id)?;
    if let Err(close_error) = window.close() {
        repository
            .update(&id, |note| {
                note.closed_at = previous_closed_at;
                Ok(())
            })
            .with_context(|| {
                format!("Could not roll back failed window close after: {close_error}")
            })?;
        return Err(close_error.into());
    }
    Ok(())
}

pub fn restore_last_closed(app: &AppHandle) -> Result<(), anyhow::Error> {
    crate::groups::restore_last_closed(app)
}

pub fn restore_all_notes(app: &AppHandle) -> Result<(), anyhow::Error> {
    crate::groups::restore_all_notes(app)
}

pub(crate) fn restore_every<T>(
    snapshots: impl IntoIterator<Item = T>,
    mut restore: impl FnMut(T) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let failures: Vec<_> = snapshots
        .into_iter()
        .filter_map(|snapshot| restore(snapshot).err().map(|error| format!("{error:#}")))
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("{}", failures.join("; "))
    }
}

pub(crate) fn include_rollback(
    error: anyhow::Error,
    rollback: anyhow::Result<()>,
) -> anyhow::Error {
    match rollback {
        Ok(()) => error,
        Err(rollback) => anyhow::anyhow!("{error:#}; rollback failed: {rollback:#}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum FoldEffect {
    Unmaximize,
    Resizable(bool),
    Size(PhysicalSize<u32>),
    Position(PhysicalPosition<i32>),
    CacheNativeGeometry,
    RestoreLiveGeometry(NoteGeometry),
}

fn change_note_fold(
    original: &StoredNote,
    candidate: &StoredNote,
    effects: &[FoldEffect],
    rollback: &[FoldEffect],
    mut save: impl FnMut(&StoredNote) -> anyhow::Result<()>,
    mut apply: impl FnMut(FoldEffect) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    save(candidate)?;
    for effect in effects {
        if let Err(error) = apply(*effect) {
            let native = restore_every(rollback.iter().copied(), &mut apply);
            let durable = save(original);
            return Err(include_rollback(
                error,
                restore_every([native, durable], |result| result),
            ));
        }
    }
    Ok(())
}

pub(crate) fn set_ungrouped_window_collapsed(
    window: &WebviewWindow,
    collapsed: bool,
) -> Result<(), anyhow::Error> {
    let id = note_id_from_label(window.label())?;
    let repository = window.state::<NoteRepository>();
    let geometries = window.state::<GeometryIndex>();
    let current = repository.get(id)?;
    if current.collapsed == collapsed {
        return Ok(());
    }

    let scale_factor = window.scale_factor()?;
    let geometry = geometries.get(id)?;
    let position = geometry.position.to_logical::<i32>(scale_factor);
    let size = geometry.size.to_logical::<u32>(scale_factor);

    let native = NoteGeometry {
        position: window.outer_position()?,
        size: window.outer_size()?,
    };
    let resizable = window.is_resizable()?;
    let mut candidate = current.clone();
    let mut effects = Vec::new();
    if collapsed {
        candidate.x = position.x;
        candidate.y = position.y;
        candidate.expanded_width = size.width.max(150);
        candidate.expanded_height = size.height.max(80);
        if window.is_maximized()? {
            effects.push(FoldEffect::Unmaximize);
        }
        effects.push(FoldEffect::Resizable(false));
        effects.push(FoldEffect::Size(
            LogicalSize::new(size.width.max(150), COLLAPSED_HEIGHT).to_physical(scale_factor),
        ));
    } else {
        let monitor = window
            .current_monitor()?
            .or(window.primary_monitor()?)
            .context("No active monitor available for expanding note")?;
        let monitor_scale = monitor.scale_factor();
        let monitor_position = monitor.position().to_logical::<i32>(monitor_scale);
        let monitor_size = monitor.size().to_logical::<u32>(monitor_scale);
        let width = current
            .expanded_width
            .clamp(150, monitor_size.width.max(150));
        let height = current
            .expanded_height
            .clamp(80, monitor_size.height.max(80));
        let max_x = monitor_position.x + monitor_size.width.saturating_sub(width) as i32;
        let max_y = monitor_position.y + monitor_size.height.saturating_sub(height) as i32;
        candidate.x = position.x.clamp(monitor_position.x, max_x);
        candidate.y = position.y.clamp(monitor_position.y, max_y);
        effects.push(FoldEffect::Resizable(true));
        effects.push(FoldEffect::Size(
            LogicalSize::new(width, height).to_physical(scale_factor),
        ));
        effects.push(FoldEffect::Position(
            LogicalPosition::new(candidate.x, candidate.y).to_physical(scale_factor),
        ));
    }
    candidate.collapsed = collapsed;
    effects.push(FoldEffect::CacheNativeGeometry);
    let rollback = [
        FoldEffect::Size(native.size),
        FoldEffect::Position(native.position),
        FoldEffect::Resizable(resizable),
        FoldEffect::RestoreLiveGeometry(geometry),
    ];
    change_note_fold(
        &current,
        &candidate,
        &effects,
        &rollback,
        |note| {
            repository
                .update(id, |stored| {
                    *stored = note.clone();
                    Ok(())
                })
                .map(|_| ())
        },
        |effect| {
            match effect {
                FoldEffect::Unmaximize => window.unmaximize()?,
                FoldEffect::Resizable(value) => window.set_resizable(value)?,
                FoldEffect::Size(value) => window.set_size(value)?,
                FoldEffect::Position(value) => window.set_position(value)?,
                FoldEffect::CacheNativeGeometry => geometries.insert(
                    id.into(),
                    NoteGeometry {
                        position: window.outer_position()?,
                        size: window.outer_size()?,
                    },
                )?,
                FoldEffect::RestoreLiveGeometry(value) => geometries.insert(id.into(), value)?,
            }
            Ok(())
        },
    )?;
    if collapsed {
        return Ok(());
    }

    window.set_focus()?;
    Ok(())
}

pub fn sorted_windows(app: &AppHandle) -> Vec<WebviewWindow> {
    let mut positions: Vec<_> = app
        .webview_windows()
        .into_iter()
        .filter(|(label, _)| label.starts_with("sticky_"))
        .filter_map(|(_label, w)| get_position_and_size(&w).ok().map(|(p, _)| (p, w)))
        .collect();

    positions.sort_by_key(|(p, _)| *p);

    positions.into_iter().map(|(_, w)| w).collect()
}

pub fn toggle_shortcuts_window(app: &AppHandle) -> Result<(), anyhow::Error> {
    if let Some(window) = app.get_webview_window(SHORTCUTS_WINDOW_LABEL) {
        window.close()?;
        return Ok(());
    }

    tauri::WebviewWindowBuilder::new(
        app,
        SHORTCUTS_WINDOW_LABEL,
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("Keyboard Shortcuts")
    .initialization_script("window.__SHORTCUTS__ = true")
    .inner_size(440.0, 560.0)
    .resizable(false)
    .maximizable(false)
    .always_on_top(true)
    .center()
    .build()?
    .set_focus()?;
    Ok(())
}

pub fn show_version_window(app: &AppHandle) -> Result<(), anyhow::Error> {
    if let Some(window) = app.get_webview_window(VERSION_WINDOW_LABEL) {
        window.show()?;
        if window.is_minimized()? {
            window.unminimize()?;
        }
        window.set_focus()?;
        return Ok(());
    }

    let init = serde_json::json!({ "installed_sha": installed_build_sha() });
    let init_script = format!("window.__VERSION_INIT__ = {init};");
    tauri::WebviewWindowBuilder::new(
        app,
        VERSION_WINDOW_LABEL,
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("Sticky Update")
    .initialization_script(init_script)
    .inner_size(420.0, 300.0)
    .resizable(false)
    .maximizable(false)
    .center()
    .build()?
    .set_focus()?;
    Ok(())
}

pub fn toggle_note_visibility(app: &AppHandle) -> Result<(), anyhow::Error> {
    let windows = sorted_windows(app);
    let should_hide = windows.iter().try_fold(false, |visible, window| {
        window.is_visible().map(|is_visible| visible || is_visible)
    })?;

    if should_hide {
        let focused_label = windows
            .iter()
            .find(|window| window.is_focused().unwrap_or(false))
            .map(|window| window.label().to_string());
        *app.state::<NoteVisibility>()
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Note visibility lock poisoned"))? = focused_label;
        for window in windows {
            window.hide()?;
        }
    } else if !windows.is_empty() {
        for window in &windows {
            window.show()?;
            if window.is_minimized()? {
                window.unminimize()?;
            }
        }
        let focused_label = app
            .state::<NoteVisibility>()
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Note visibility lock poisoned"))?
            .take();
        focused_label
            .and_then(|label| app.get_webview_window(&label))
            .unwrap_or_else(|| windows[0].clone())
            .set_focus()?;
    }

    Ok(())
}

pub fn cycle_focus(app: &AppHandle, reverse: bool) -> Result<(), anyhow::Error> {
    let mut sorted_windows = sorted_windows(app);
    if reverse {
        sorted_windows.reverse();
    }

    let focused_index = sorted_windows
        .iter()
        .position(|w| w.is_focused().unwrap_or(false))
        .context("No window currently focused")?;

    let next_window_index = (focused_index + 1) % sorted_windows.len();

    sorted_windows[next_window_index]
        .set_focus()
        .context("Could not focus window")
}

pub fn change_note_font_size(
    app: &AppHandle,
    window: &WebviewWindow,
    increase: bool,
) -> Result<(), anyhow::Error> {
    let id = note_id_from_label(window.label())?;
    let repository = app.state::<NoteRepository>();
    let current = repository.get(id)?;
    let delta = if increase {
        i64::from(FONT_SIZE_STEP)
    } else {
        -i64::from(FONT_SIZE_STEP)
    };
    let font_size = clamp_font_size(i64::from(current.font_size) + delta);
    debug_assert!((MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&font_size));

    let settings = app.state::<MenuSettings>();
    let previous_default = settings.set_default_font_size(font_size)?;
    if let Err(save_error) = save_settings(app) {
        settings.set_default_font_size(previous_default)?;
        let rollback_error = save_settings(app).err();
        if let Some(rollback_error) = rollback_error {
            bail!(
                "Could not save the font-size default ({save_error:#}) or restore it ({rollback_error:#})"
            );
        }
        return Err(save_error.context("Could not save the font-size default"));
    }

    if font_size != current.font_size {
        if let Err(note_error) = repository.update(id, |note| {
            note.font_size = font_size;
            Ok(())
        }) {
            settings.set_default_font_size(previous_default)?;
            if let Err(rollback_error) = save_settings(app) {
                bail!(
                    "Could not save note font size ({note_error:#}) or restore its default ({rollback_error:#})"
                );
            }
            return Err(note_error.context("Could not save note font size"));
        }
    }

    window.emit_to(
        EventTarget::webview_window(window.label()),
        "set_font_size",
        font_size,
    )?;
    Ok(())
}

#[cfg(test)]
mod snap_tests {
    use super::*;

    #[test]
    fn rollback_attempts_every_snapshot_and_reports_every_failure() {
        let mut attempted = Vec::new();
        let error = restore_every(0..4, |snapshot| {
            attempted.push(snapshot);
            if snapshot == 0 || snapshot == 2 {
                bail!("snapshot {snapshot}");
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!(attempted, [0, 1, 2, 3]);
        assert_eq!(error.to_string(), "snapshot 0; snapshot 2");
        assert!(restore_every([1, 2], |_| Ok(())).is_ok());
    }

    #[test]
    fn fold_and_unfold_restore_native_live_and_saved_state_after_each_failed_step() {
        for collapsed in [true, false] {
            let mut original = StoredNote::new();
            original.collapsed = !collapsed;
            let mut candidate = original.clone();
            candidate.collapsed = collapsed;
            candidate.x += 20;
            let prior = NoteGeometry {
                position: PhysicalPosition::new(50, 70),
                size: PhysicalSize::new(300, if collapsed { 250 } else { 24 }),
            };
            let live_prior = NoteGeometry {
                position: PhysicalPosition::new(51, 71),
                ..prior
            };
            let target = NoteGeometry {
                position: PhysicalPosition::new(100, 100),
                size: PhysicalSize::new(300, if collapsed { 24 } else { 250 }),
            };
            let mut effects = vec![
                FoldEffect::Resizable(!collapsed),
                FoldEffect::Size(target.size),
            ];
            if collapsed {
                effects.insert(0, FoldEffect::Unmaximize);
            } else {
                effects.push(FoldEffect::Position(target.position));
            }
            effects.push(FoldEffect::CacheNativeGeometry);
            let rollback = [
                FoldEffect::Size(prior.size),
                FoldEffect::Position(prior.position),
                FoldEffect::Resizable(collapsed),
                FoldEffect::RestoreLiveGeometry(live_prior),
            ];
            for failed_step in 0..effects.len() {
                let dir =
                    std::env::temp_dir().join(format!("stickymd-fold-{}", uuid::Uuid::new_v4()));
                let repository = NoteRepository::load_from_dir(&dir).unwrap();
                let id = repository.active().unwrap().remove(0).id;
                repository
                    .update(&id, |note| {
                        *note = original.clone();
                        note.id = id.clone();
                        Ok(())
                    })
                    .unwrap();
                let before = repository.get(&id).unwrap();
                let mut native = prior;
                let mut live = live_prior;
                let mut resizable = collapsed;
                let mut maximized = collapsed;
                let mut step = 0;
                let result = change_note_fold(
                    &before,
                    &candidate,
                    &effects,
                    &rollback,
                    |value| {
                        repository
                            .update(&id, |note| {
                                *note = value.clone();
                                note.id = id.clone();
                                Ok(())
                            })
                            .map(|_| ())
                    },
                    |effect| {
                        match effect {
                            FoldEffect::Unmaximize => {
                                maximized = false;
                                native.position = PhysicalPosition::new(0, 0);
                            }
                            FoldEffect::Resizable(value) => resizable = value,
                            FoldEffect::Size(value) => native.size = value,
                            FoldEffect::Position(value) => native.position = value,
                            FoldEffect::CacheNativeGeometry => live = native,
                            FoldEffect::RestoreLiveGeometry(value) => live = value,
                        }
                        let fail = step == failed_step;
                        step += 1;
                        if fail {
                            bail!("injected native failure");
                        }
                        Ok(())
                    },
                );
                assert!(result.is_err());
                assert_eq!(native, prior);
                assert_eq!(live, live_prior);
                assert_eq!(resizable, collapsed);
                if collapsed {
                    assert!(!maximized);
                }
                let reopened = NoteRepository::load_from_dir(&dir)
                    .unwrap()
                    .get(&id)
                    .unwrap();
                assert_eq!(
                    serde_json::to_value(reopened).unwrap(),
                    serde_json::to_value(&before).unwrap()
                );
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    }

    #[test]
    fn fold_save_failure_has_no_native_effects_and_rollback_failures_are_combined() {
        let original = StoredNote::new();
        let mut candidate = original.clone();
        candidate.collapsed = true;
        let mut native_calls = Vec::new();
        let error = change_note_fold(
            &original,
            &candidate,
            &[FoldEffect::Resizable(false)],
            &[],
            |_| bail!("save failed"),
            |effect| {
                native_calls.push(effect);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "save failed");
        assert!(native_calls.is_empty());
        let mut saves = 0;
        let rollback = [
            FoldEffect::Resizable(true),
            FoldEffect::Position(PhysicalPosition::new(1, 2)),
        ];
        let error = change_note_fold(
            &original,
            &candidate,
            &[FoldEffect::Resizable(false)],
            &rollback,
            |_| {
                saves += 1;
                if saves == 2 {
                    bail!("saved state rollback");
                }
                Ok(())
            },
            |effect| {
                native_calls.push(effect);
                bail!("native {effect:?}");
            },
        )
        .unwrap_err();
        assert_eq!(native_calls.len(), 3);
        assert_eq!(saves, 2);
        let error = error.to_string();
        assert!(error.contains("Resizable(false)"));
        assert!(error.contains("Resizable(true)"));
        assert!(error.contains("Position"));
        assert!(error.contains("saved state rollback"));
    }

    #[test]
    fn full_and_partial_snap_targets_keep_the_other_axis_and_use_nearest_edges() {
        let origin = PhysicalPosition::new(400, 300);
        let size = PhysicalSize::new(100, 80);
        let others = [
            (PhysicalPosition::new(200, 300), size),
            (PhysicalPosition::new(600, 300), size),
            (PhysicalPosition::new(400, 100), size),
            (PhysicalPosition::new(400, 500), size),
            (PhysicalPosition::new(350, 0), PhysicalSize::new(20, 20)),
        ];
        for (direction, full, partial) in [
            (Direction::Left, (320, 300), (390, 300)),
            (Direction::Right, (480, 300), (480, 300)),
            (Direction::Up, (400, 200), (400, 200)),
            (Direction::Down, (400, 400), (400, 400)),
        ] {
            for (partial_snap, expected) in [(false, full), (true, partial)] {
                assert_eq!(
                    snap_target(
                        origin,
                        size,
                        direction,
                        partial_snap,
                        PhysicalPosition::new(0, 0),
                        PhysicalSize::new(1000, 800),
                        None,
                        &others
                    ),
                    PhysicalPosition::new(expected.0, expected.1)
                );
            }
        }
    }

    #[test]
    fn snap_targets_use_monitor_edges_and_preserve_cross_monitor_behavior() {
        let size = PhysicalSize::new(100, 80);
        for partial in [false, true] {
            for (direction, expected) in [
                (Direction::Left, (20, 300)),
                (Direction::Right, (880, 300)),
                (Direction::Up, (400, 20)),
                (Direction::Down, (400, 700)),
            ] {
                assert_eq!(
                    snap_target(
                        PhysicalPosition::new(400, 300),
                        size,
                        direction,
                        partial,
                        PhysicalPosition::new(0, 0),
                        PhysicalSize::new(1000, 800),
                        None,
                        &[]
                    ),
                    PhysicalPosition::new(expected.0, expected.1)
                );
                assert_eq!(
                    snap_target(
                        PhysicalPosition::new(400, 300),
                        size,
                        direction,
                        partial,
                        PhysicalPosition::new(0, 0),
                        PhysicalSize::new(1000, 800),
                        Some(PhysicalPosition::new(-1000, 100)),
                        &[]
                    ),
                    PhysicalPosition::new(-980, 120)
                );
            }
            let at_edge = PhysicalPosition::new(20, 300);
            assert_eq!(
                snap_target(
                    at_edge,
                    size,
                    Direction::Left,
                    partial,
                    PhysicalPosition::new(0, 0),
                    PhysicalSize::new(1000, 800),
                    None,
                    &[]
                ),
                at_edge
            );
        }
    }
}
