use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, MutexGuard},
};

use anyhow::{bail, Context};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow,
};
use tauri_plugin_log::log;

use crate::{
    save_load::{current_time_millis, note_id_from_label, NoteRepository, StoredGroup, StoredNote},
    windows::{
        close_ungrouped_window_and_archive, open_missing_active_notes, open_sticky,
        set_ungrouped_window_collapsed, sorted_windows, GeometryIndex, NoteGeometry,
    },
};

const COLLAPSED_HEIGHT: u32 = 24;
const GROUP_GAP: u32 = 12;
const RESET_MARGIN: i32 = 20;
const DRAG_DETACH_THRESHOLD: i32 = 4;
const NATIVE_POSITION_ROUNDING_TOLERANCE: i32 = 2;

#[derive(Default)]
pub(crate) struct GroupRuntimeState {
    drag_origins: HashMap<String, PhysicalPosition<i32>>,
    completed_drag_origins: HashMap<String, PhysicalPosition<i32>>,
    programmatic_positions: HashMap<String, PhysicalPosition<i32>>,
}

impl GroupRuntimeState {
    fn begin_user_drag(&mut self, id: String, origin: PhysicalPosition<i32>) {
        self.completed_drag_origins.remove(&id);
        self.programmatic_positions.remove(&id);
        self.drag_origins.insert(id, origin);
    }

    fn cancel_user_drag(&mut self, id: &str) {
        self.drag_origins.remove(id);
    }

    fn complete_user_drag(&mut self, id: &str) -> anyhow::Result<()> {
        let origin = self
            .drag_origins
            .remove(id)
            .with_context(|| format!("No active drag origin for note {id}"))?;
        self.completed_drag_origins.insert(id.to_string(), origin);
        Ok(())
    }

    fn take_completed_drag(&mut self, id: &str) -> Option<PhysicalPosition<i32>> {
        self.completed_drag_origins.remove(id)
    }

    fn record_programmatic_position(&mut self, id: String, position: PhysicalPosition<i32>) {
        self.completed_drag_origins.remove(&id);
        self.programmatic_positions.insert(id, position);
    }
}

#[derive(Default)]
pub struct GroupRuntime(Mutex<GroupRuntimeState>);

impl GroupRuntime {
    pub(crate) fn lock(&self) -> anyhow::Result<MutexGuard<'_, GroupRuntimeState>> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("Group runtime lock poisoned"))
    }
}

struct WindowSnapshot {
    id: String,
    window: WebviewWindow,
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
}

struct GroupLayout {
    snapshots: Vec<WindowSnapshot>,
    targets: Vec<LogicalPosition<i32>>,
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

    fn center_twice(self) -> (i64, i64) {
        (2 * self.x + self.width, 2 * self.y + self.height)
    }

    fn contains_center_twice(self, (x, y): (i64, i64)) -> bool {
        2 * self.x <= x && x < 2 * self.right() && 2 * self.y <= y && y < 2 * self.bottom()
    }
}

fn get_focused_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.webview_windows()
        .into_iter()
        .find(|(label, window)| {
            label.starts_with("sticky_") && window.is_focused().unwrap_or(false)
        })
        .map(|(_, window)| window)
}

fn snapshots_for_ids(app: &AppHandle, ids: &[String]) -> anyhow::Result<Vec<WindowSnapshot>> {
    let windows = app.webview_windows();
    let geometries = app.state::<GeometryIndex>();
    ids.iter()
        .map(|id| {
            let window = windows
                .get(&format!("sticky_{id}"))
                .with_context(|| format!("Active note {id} did not have an open window"))?;
            let geometry = geometries.get(id)?;
            Ok(WindowSnapshot {
                id: id.clone(),
                window: window.clone(),
                position: geometry.position,
                size: geometry.size,
            })
        })
        .collect()
}

fn visual_order(ids: &HashSet<String>, geometries: &GeometryIndex) -> anyhow::Result<Vec<String>> {
    let mut ordered = ids
        .iter()
        .map(|id| Ok((id.clone(), geometries.get(id)?)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    ordered.sort_by(|a, b| {
        let a_position = a.1.position;
        let b_position = b.1.position;
        (a_position.y, a_position.x, &a.0).cmp(&(b_position.y, b_position.x, &b.0))
    });
    Ok(ordered.into_iter().map(|(id, _)| id).collect())
}

fn ids_on_anchor_monitor_side(
    anchor_id: &str,
    candidate_ids: &[String],
    geometries: &GeometryIndex,
    monitor: WindowRect,
    work_area: WindowRect,
) -> anyhow::Result<Vec<String>> {
    let anchor_geometry = geometries.get(anchor_id)?;
    let anchor_center =
        WindowRect::from_physical(anchor_geometry.position, anchor_geometry.size).center_twice();
    if !monitor.contains_center_twice(anchor_center) {
        bail!("Selected parent center was outside its current monitor");
    }
    let midpoint_twice = 2 * work_area.x + work_area.width;
    let anchor_is_left = anchor_center.0 < midpoint_twice;
    let mut eligible = HashSet::new();
    for id in candidate_ids {
        let geometry = geometries.get(id)?;
        let center = WindowRect::from_physical(geometry.position, geometry.size).center_twice();
        if monitor.contains_center_twice(center) && (center.0 < midpoint_twice) == anchor_is_left {
            eligible.insert(id.clone());
        }
    }
    visual_order(&eligible, geometries)
}

fn arranged_positions(
    origin: LogicalPosition<i32>,
    heights: &[u32],
) -> anyhow::Result<Vec<LogicalPosition<i32>>> {
    let mut y = i64::from(origin.y);
    heights
        .iter()
        .enumerate()
        .map(|(index, height)| {
            let position = LogicalPosition::new(
                origin.x,
                i32::try_from(y).context("Group position exceeded platform limits")?,
            );
            if index + 1 < heights.len() {
                y = y
                    .checked_add(i64::from(*height) + i64::from(GROUP_GAP))
                    .context("Group layout height overflowed")?;
            }
            Ok(position)
        })
        .collect()
}

fn durable_note_height(note: &StoredNote) -> u32 {
    if note.collapsed {
        COLLAPSED_HEIGHT
    } else {
        note.expanded_height.max(80)
    }
}

fn positions_after_changed_note(
    origin: LogicalPosition<i32>,
    changed_height: u32,
    later_heights: &[u32],
) -> anyhow::Result<Vec<LogicalPosition<i32>>> {
    let heights = std::iter::once(changed_height)
        .chain(later_heights.iter().copied())
        .collect::<Vec<_>>();
    Ok(arranged_positions(origin, &heights)?
        .into_iter()
        .skip(1)
        .collect())
}

fn requested_physical_position(
    snapshot: &WindowSnapshot,
    target: LogicalPosition<i32>,
) -> anyhow::Result<PhysicalPosition<i32>> {
    Ok(target.to_physical(snapshot.window.scale_factor()?))
}

#[derive(Debug, PartialEq, Eq)]
enum PositionSettlement {
    AdoptProgrammatic(PhysicalPosition<i32>),
    ExternalMove,
    Unchanged,
}

fn positions_within_rounding_tolerance(
    requested: PhysicalPosition<i32>,
    observed: PhysicalPosition<i32>,
) -> bool {
    (requested.x - observed.x)
        .abs()
        .max((requested.y - observed.y).abs())
        <= NATIVE_POSITION_ROUNDING_TOLERANCE
}

fn position_settlement(
    programmatic_positions: &mut HashMap<String, PhysicalPosition<i32>>,
    id: &str,
    observed: PhysicalPosition<i32>,
    durable: LogicalPosition<i32>,
    scale: f64,
) -> PositionSettlement {
    if let Some(requested) = programmatic_positions.get(id).copied() {
        if positions_within_rounding_tolerance(requested, observed) {
            programmatic_positions.remove(id);
            PositionSettlement::AdoptProgrammatic(observed)
        } else {
            PositionSettlement::ExternalMove
        }
    } else if observed.to_logical::<i32>(scale) != durable {
        PositionSettlement::ExternalMove
    } else {
        PositionSettlement::Unchanged
    }
}

fn active_group_ids(
    repository: &NoteRepository,
    group: &StoredGroup,
    excluded: &HashSet<String>,
) -> anyhow::Result<Vec<String>> {
    let active: HashSet<_> = repository
        .active()?
        .into_iter()
        .map(|note| note.id)
        .collect();
    Ok(group
        .note_ids
        .iter()
        .filter(|id| active.contains(*id) && !excluded.contains(*id))
        .cloned()
        .collect())
}

fn layout_for_ids_at_origin(
    app: &AppHandle,
    ids: &[String],
    origin_override: Option<LogicalPosition<i32>>,
) -> anyhow::Result<GroupLayout> {
    let snapshots = snapshots_for_ids(app, ids)?;
    let first = snapshots.first().context("Group had no active members")?;
    let origin = origin_override.unwrap_or(
        first
            .position
            .to_logical::<i32>(first.window.scale_factor()?),
    );
    let repository = app.state::<NoteRepository>();
    let heights = snapshots
        .iter()
        .map(|snapshot| {
            let note = repository.get(&snapshot.id)?;
            Ok(durable_note_height(&note))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let targets = arranged_positions(origin, &heights)?;
    Ok(GroupLayout { snapshots, targets })
}

fn restore_snapshots(
    snapshots: &[WindowSnapshot],
    geometries: &GeometryIndex,
    runtime: &mut GroupRuntimeState,
) {
    for snapshot in snapshots {
        if let Err(error) = snapshot.window.set_size(snapshot.size) {
            log::error!("Could not restore note {} size: {error}", snapshot.id);
        }
        let position_restored = if let Err(error) = snapshot.window.set_position(snapshot.position)
        {
            log::error!("Could not restore note {} position: {error}", snapshot.id);
            false
        } else {
            true
        };
        let _ = geometries.insert(
            snapshot.id.clone(),
            NoteGeometry {
                position: snapshot.position,
                size: snapshot.size,
            },
        );
        if position_restored {
            runtime.record_programmatic_position(snapshot.id.clone(), snapshot.position);
        }
    }
}

fn apply_layout(
    layout: &GroupLayout,
    geometries: &GeometryIndex,
    runtime: &mut GroupRuntimeState,
) -> anyhow::Result<()> {
    for (index, (snapshot, target)) in layout.snapshots.iter().zip(&layout.targets).enumerate() {
        let current = snapshot
            .position
            .to_logical::<i32>(snapshot.window.scale_factor()?);
        let requested = requested_physical_position(snapshot, *target)?;
        if current != *target {
            if let Err(error) = snapshot.window.set_position(*target) {
                restore_snapshots(&layout.snapshots[..index], geometries, runtime);
                return Err(error)
                    .with_context(|| format!("Could not position group member {}", snapshot.id));
            }
        }
        if let Err(error) = geometries.set_position(&snapshot.id, requested) {
            restore_snapshots(&layout.snapshots[..=index], geometries, runtime);
            return Err(error)
                .with_context(|| format!("Could not cache group member {}", snapshot.id));
        }
        if current != *target {
            runtime.record_programmatic_position(snapshot.id.clone(), requested);
        }
    }
    Ok(())
}

fn persist_layout_positions(
    store: &mut crate::save_load::NoteStore,
    layout: &GroupLayout,
) -> anyhow::Result<()> {
    for (snapshot, target) in layout.snapshots.iter().zip(&layout.targets) {
        let note = store
            .notes
            .get_mut(&snapshot.id)
            .with_context(|| format!("Cannot position missing group member {}", snapshot.id))?;
        note.x = target.x;
        note.y = target.y;
    }
    Ok(())
}

pub fn link_notes_on_this_side_below(
    app: &AppHandle,
    parent: &WebviewWindow,
) -> anyhow::Result<()> {
    let runtime_state = app.state::<GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    open_missing_active_notes(app)?;
    let repository = app.state::<NoteRepository>();
    let geometries = app.state::<GeometryIndex>();
    let parent_id = note_id_from_label(parent.label())?.to_string();
    let existing_group = repository.group_for_note(&parent_id)?;
    let existing_group_ids: HashSet<_> = existing_group
        .as_ref()
        .map(|group| group.note_ids.iter().cloned().collect())
        .unwrap_or_default();
    let grouped_ids: HashSet<_> = repository
        .all_groups()?
        .into_iter()
        .flat_map(|group| group.note_ids)
        .collect();
    let independent_ids: Vec<_> = repository
        .active()?
        .into_iter()
        .map(|note| note.id)
        .filter(|id| !grouped_ids.contains(id) && id != &parent_id)
        .collect();
    let monitor = parent
        .current_monitor()?
        .context("Selected parent did not have a current monitor")?;
    let monitor_rect = WindowRect::from_physical(*monitor.position(), *monitor.size());
    let work_area =
        WindowRect::from_physical(monitor.work_area().position, monitor.work_area().size);
    let eligible_independent = ids_on_anchor_monitor_side(
        &parent_id,
        &independent_ids,
        &geometries,
        monitor_rect,
        work_area,
    )?;
    let active_ids: HashSet<_> = repository
        .active()?
        .into_iter()
        .map(|note| note.id)
        .collect();
    let mut other_ids: HashSet<_> = eligible_independent.into_iter().collect();
    other_ids.extend(
        existing_group_ids
            .iter()
            .filter(|id| active_ids.contains(*id) && *id != &parent_id)
            .cloned(),
    );
    let mut order = vec![parent_id.clone()];
    order.extend(visual_order(&other_ids, &geometries)?);
    if order.len() < 2 {
        bail!("No independent notes are available to link on this monitor side");
    }

    let parent_geometry = geometries.get(&parent_id)?;
    let parent_origin = parent_geometry
        .position
        .to_logical::<i32>(parent.scale_factor()?);
    let layout = layout_for_ids_at_origin(app, &order, Some(parent_origin))?;
    apply_layout(&layout, &geometries, &mut runtime)?;

    let group_id = existing_group
        .as_ref()
        .map(|group| group.id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut durable_order = order.clone();
    if let Some(group) = &existing_group {
        durable_order.extend(
            group
                .note_ids
                .iter()
                .filter(|id| !active_ids.contains(*id))
                .cloned(),
        );
    }
    let persist = repository.mutate(|store| {
        persist_layout_positions(store, &layout)?;
        store.groups.insert(
            group_id.clone(),
            StoredGroup {
                id: group_id.clone(),
                note_ids: durable_order.clone(),
            },
        );
        Ok(())
    });
    if let Err(error) = persist {
        restore_snapshots(&layout.snapshots, &geometries, &mut runtime);
        return Err(error.context("Could not persist linked group"));
    }
    Ok(())
}

pub fn link_notes_on_this_side_below_focused(app: &AppHandle) -> anyhow::Result<()> {
    let parent = get_focused_window(app).context("No note currently focused")?;
    link_notes_on_this_side_below(app, &parent)
}

pub fn unlink_group_for_focused(app: &AppHandle) -> anyhow::Result<()> {
    let runtime_state = app.state::<GroupRuntime>();
    let _runtime = runtime_state.lock()?;
    let window = get_focused_window(app).context("No note currently focused")?;
    let id = note_id_from_label(window.label())?;
    let repository = app.state::<NoteRepository>();
    let group = repository
        .group_for_note(id)?
        .context("The focused note is not in a linked group")?;
    repository.mutate(|store| {
        store.groups.remove(&group.id);
        Ok(())
    })
}

fn restore_stored_geometry(
    window: &WebviewWindow,
    note: &StoredNote,
    geometries: &GeometryIndex,
    runtime: &mut GroupRuntimeState,
) {
    let result = (|| -> anyhow::Result<()> {
        let height = if note.collapsed {
            COLLAPSED_HEIGHT
        } else {
            note.expanded_height.max(80)
        };
        let requested_size = LogicalSize::new(note.expanded_width.max(150), height);
        let requested_position = LogicalPosition::new(note.x, note.y);
        window.set_size(requested_size)?;
        window.set_position(requested_position)?;
        let scale = window.scale_factor()?;
        let geometry = NoteGeometry {
            position: requested_position.to_physical(scale),
            size: requested_size.to_physical(scale),
        };
        geometries.insert(note.id.clone(), geometry)?;
        runtime.record_programmatic_position(note.id.clone(), geometry.position);
        Ok(())
    })();
    if let Err(error) = result {
        log::error!("Could not restore note {} geometry: {error:#}", note.id);
    }
}

fn persist_group_detachment(
    store: &mut crate::save_load::NoteStore,
    group_id: &str,
    id: &str,
    position: LogicalPosition<i32>,
    size: LogicalSize<u32>,
) -> anyhow::Result<()> {
    let note = store
        .notes
        .get_mut(id)
        .with_context(|| format!("Cannot detach missing note {id}"))?;
    note.x = position.x;
    note.y = position.y;
    if !note.collapsed {
        note.expanded_width = size.width.max(150);
        note.expanded_height = size.height.max(80);
    }
    let stored_group = store
        .groups
        .get_mut(group_id)
        .with_context(|| format!("Cannot detach from missing group {group_id}"))?;
    stored_group.note_ids.retain(|member| member != id);
    if stored_group.note_ids.len() < 2 {
        store.groups.remove(group_id);
    }
    Ok(())
}

fn detach_member(
    window: &WebviewWindow,
    group: &StoredGroup,
    geometry: NoteGeometry,
    runtime: &mut GroupRuntimeState,
) -> anyhow::Result<()> {
    let app = window.app_handle();
    let id = note_id_from_label(window.label())?.to_string();
    let repository = app.state::<NoteRepository>();
    let geometries = app.state::<GeometryIndex>();
    let previous = repository.get(&id)?;
    let scale = window.scale_factor()?;
    let position = geometry.position.to_logical::<i32>(scale);
    let size = geometry.size.to_logical::<u32>(scale);
    let persist =
        repository.mutate(|store| persist_group_detachment(store, &group.id, &id, position, size));
    if let Err(error) = persist {
        restore_stored_geometry(window, &previous, &geometries, runtime);
        return Err(error.context("Could not persist group detachment"));
    }
    Ok(())
}

fn drag_exceeds_threshold(start: LogicalPosition<i32>, end: LogicalPosition<i32>) -> bool {
    (end.x - start.x).abs().max((end.y - start.y).abs()) > DRAG_DETACH_THRESHOLD
}

pub fn run_window_drag<F>(window: &WebviewWindow, drag: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()>,
{
    let app = window.app_handle();
    let runtime_state = app.state::<GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    let id = note_id_from_label(window.label())?.to_string();
    let origin = app.state::<GeometryIndex>().get(&id)?.position;
    runtime.begin_user_drag(id.clone(), origin);
    if let Err(error) = drag() {
        runtime.cancel_user_drag(&id);
        return Err(error);
    }
    runtime.complete_user_drag(&id)
}

fn resize_group_member(
    window: &WebviewWindow,
    group: &StoredGroup,
    old_note: &StoredNote,
    target_size: LogicalSize<u32>,
    collapsed: Option<bool>,
    runtime: &mut GroupRuntimeState,
) -> anyhow::Result<()> {
    let app = window.app_handle();
    let id = note_id_from_label(window.label())?.to_string();
    let repository = app.state::<NoteRepository>();
    let geometries = app.state::<GeometryIndex>();
    let active_ids = active_group_ids(&repository, group, &HashSet::new())?;
    let index = active_ids
        .iter()
        .position(|member| member == &id)
        .context("Group did not contain the resized active note")?;
    let selected_geometry = geometries.get(&id)?;
    let scale = window.scale_factor()?;
    let previous_logical_size = LogicalSize::new(
        old_note.expanded_width.max(150),
        if old_note.collapsed {
            COLLAPSED_HEIGHT
        } else {
            old_note.expanded_height.max(80)
        },
    );
    let selected_snapshot = WindowSnapshot {
        id: id.clone(),
        window: window.clone(),
        position: selected_geometry.position,
        size: previous_logical_size.to_physical(scale),
    };
    let old_size = previous_logical_size;
    let later_snapshots = snapshots_for_ids(app, &active_ids[index + 1..])?;
    let later_heights = later_snapshots
        .iter()
        .map(|snapshot| {
            repository
                .get(&snapshot.id)
                .map(|note| durable_note_height(&note))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let targets = positions_after_changed_note(
        LogicalPosition::new(old_note.x, old_note.y),
        target_size.height,
        &later_heights,
    )?;

    let native = (|| -> anyhow::Result<()> {
        if collapsed == Some(true) && window.is_maximized()? {
            window.unmaximize()?;
        }
        if let Some(collapsed) = collapsed {
            window.set_resizable(!collapsed)?;
        }
        window.set_size(target_size)?;
        geometries.set_size(&id, target_size.to_physical(scale))?;
        for (snapshot, target) in later_snapshots.iter().zip(&targets) {
            snapshot.window.set_position(*target)?;
            let requested = requested_physical_position(snapshot, *target)?;
            geometries.set_position(&snapshot.id, requested)?;
            runtime.record_programmatic_position(snapshot.id.clone(), requested);
        }
        Ok(())
    })();
    if let Err(error) = native {
        restore_snapshots(&later_snapshots, &geometries, runtime);
        restore_snapshots(
            std::slice::from_ref(&selected_snapshot),
            &geometries,
            runtime,
        );
        let _ = window.set_resizable(!old_note.collapsed);
        return Err(error.context("Could not resize linked group member"));
    }

    let persist = repository.mutate(|store| {
        let note = store
            .notes
            .get_mut(&id)
            .with_context(|| format!("Cannot resize missing note {id}"))?;
        if collapsed == Some(true) {
            note.expanded_width = old_size.width.max(150);
            note.expanded_height = old_size.height.max(80);
        } else if collapsed != Some(false) || !old_note.collapsed {
            note.expanded_width = target_size.width.max(150);
            note.expanded_height = target_size.height.max(80);
        }
        if let Some(collapsed) = collapsed {
            note.collapsed = collapsed;
        }
        for (snapshot, target) in later_snapshots.iter().zip(&targets) {
            let later = store
                .notes
                .get_mut(&snapshot.id)
                .with_context(|| format!("Cannot reflow missing group member {}", snapshot.id))?;
            later.x = target.x;
            later.y = target.y;
        }
        Ok(())
    });
    if let Err(error) = persist {
        restore_snapshots(&later_snapshots, &geometries, runtime);
        restore_snapshots(
            std::slice::from_ref(&selected_snapshot),
            &geometries,
            runtime,
        );
        let _ = window.set_resizable(!old_note.collapsed);
        return Err(error.context("Could not persist linked group resize"));
    }
    Ok(())
}

fn set_group_member_collapsed(
    window: &WebviewWindow,
    collapsed: bool,
    runtime: &mut GroupRuntimeState,
) -> anyhow::Result<()> {
    let app = window.app_handle();
    let id = note_id_from_label(window.label())?;
    let repository = app.state::<NoteRepository>();
    let current = repository.get(id)?;
    if current.collapsed == collapsed {
        return Ok(());
    }
    let group = repository
        .group_for_note(id)?
        .context("Note was no longer in a linked group")?;
    let current_size = app
        .state::<GeometryIndex>()
        .get(id)?
        .size
        .to_logical::<u32>(window.scale_factor()?);
    let target = LogicalSize::new(
        current_size.width.max(150),
        if collapsed {
            COLLAPSED_HEIGHT
        } else {
            current.expanded_height.max(80)
        },
    );
    resize_group_member(window, &group, &current, target, Some(collapsed), runtime)?;
    if !collapsed {
        window.set_focus()?;
    }
    Ok(())
}

pub fn set_window_collapsed(window: &WebviewWindow, collapsed: bool) -> anyhow::Result<()> {
    let app = window.app_handle();
    let runtime_state = app.state::<GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    let id = note_id_from_label(window.label())?;
    if app.state::<NoteRepository>().group_for_note(id)?.is_some() {
        set_group_member_collapsed(window, collapsed, &mut runtime)
    } else {
        set_ungrouped_window_collapsed(window, collapsed)
    }
}

pub fn resize_note_height(window: &WebviewWindow, height: u32) -> anyhow::Result<()> {
    let app = window.app_handle();
    let runtime_state = app.state::<GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    let id = note_id_from_label(window.label())?;
    let repository = app.state::<NoteRepository>();
    let current = repository.get(id)?;
    if current.collapsed {
        return Ok(());
    }
    let geometry = app.state::<GeometryIndex>().get(id)?;
    let size = geometry.size.to_logical::<u32>(window.scale_factor()?);
    let target = LogicalSize::new(size.width.max(150), height.max(80));
    if let Some(group) = repository.group_for_note(id)? {
        resize_group_member(window, &group, &current, target, None, &mut runtime)
    } else {
        let snapshot = WindowSnapshot {
            id: id.to_string(),
            window: window.clone(),
            position: geometry.position,
            size: geometry.size,
        };
        if let Err(error) = window.set_size(target) {
            return Err(error.into());
        }
        let physical_size = window.outer_size()?;
        app.state::<GeometryIndex>().set_size(id, physical_size)?;
        let logical = physical_size.to_logical::<u32>(window.scale_factor()?);
        if let Err(error) = repository.update(id, |note| {
            note.expanded_width = logical.width.max(150);
            note.expanded_height = logical.height.max(80);
            Ok(())
        }) {
            restore_snapshots(&[snapshot], &app.state::<GeometryIndex>(), &mut runtime);
            return Err(error.context("Could not persist note height"));
        }
        Ok(())
    }
}

pub fn settle_window_geometry(window: &WebviewWindow) -> anyhow::Result<()> {
    let app = window.app_handle();
    let runtime_state = app.state::<GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    let id = note_id_from_label(window.label())?.to_string();
    if runtime.drag_origins.contains_key(&id) {
        return Ok(());
    }
    let geometries = app.state::<GeometryIndex>();
    let geometry = geometries.get(&id)?;
    let scale = window.scale_factor()?;
    let position = geometry.position.to_logical::<i32>(scale);
    let size = geometry.size.to_logical::<u32>(scale);
    let repository = app.state::<NoteRepository>();
    let current = repository.get(&id)?;
    let group = repository.group_for_note(&id)?;
    if let Some(origin) = runtime.take_completed_drag(&id) {
        let start = origin.to_logical::<i32>(scale);
        let end = geometry.position.to_logical::<i32>(scale);
        if let Some(group) = &group {
            if drag_exceeds_threshold(start, end) {
                return detach_member(window, group, geometry, &mut runtime);
            }
            if geometry.position != origin {
                window.set_position(origin)?;
                geometries.set_position(&id, origin)?;
                runtime.record_programmatic_position(id.clone(), origin);
            }
        } else {
            repository.update_geometry_if_changed(
                &id,
                position.x,
                position.y,
                size.width,
                size.height,
            )?;
            return Ok(());
        }
    } else {
        match position_settlement(
            &mut runtime.programmatic_positions,
            &id,
            geometry.position,
            LogicalPosition::new(current.x, current.y),
            scale,
        ) {
            PositionSettlement::AdoptProgrammatic(observed) => {
                if position.x != current.x || position.y != current.y {
                    repository.update(&id, |note| {
                        note.x = position.x;
                        note.y = position.y;
                        Ok(())
                    })?;
                }
                geometries.set_position(&id, observed)?;
            }
            PositionSettlement::ExternalMove => {
                if current.pinned {
                    window.set_position(LogicalPosition::new(current.x, current.y))?;
                    let requested = LogicalPosition::new(current.x, current.y).to_physical(scale);
                    geometries.set_position(&id, requested)?;
                    runtime.record_programmatic_position(id.clone(), requested);
                }
            }
            PositionSettlement::Unchanged => {}
        }
    }
    if group.is_none() {
        repository.update_size_if_changed(&id, size.width, size.height)?;
        return Ok(());
    }
    let group = group.context("Note was no longer in a linked group")?;
    if current.collapsed
        || (size.width == current.expanded_width && size.height == current.expanded_height)
    {
        return Ok(());
    }
    resize_group_member(
        window,
        &group,
        &current,
        LogicalSize::new(size.width.max(150), size.height.max(80)),
        None,
        &mut runtime,
    )
}

fn close_group_member(
    window: &WebviewWindow,
    runtime: &mut GroupRuntimeState,
) -> anyhow::Result<()> {
    let app = window.app_handle();
    let id = note_id_from_label(window.label())?.to_string();
    let repository = app.state::<NoteRepository>();
    let previous = repository.get(&id)?;
    let group = repository
        .group_for_note(&id)?
        .context("Note was no longer in a linked group")?;
    let active_before = active_group_ids(&repository, &group, &HashSet::new())?;
    let removed_top = active_before.first().is_some_and(|member| member == &id);
    let remaining = active_group_ids(&repository, &group, &HashSet::from([id.clone()]))?;
    let layout = if remaining.is_empty() {
        None
    } else {
        Some(layout_for_ids_at_origin(
            app,
            &remaining,
            removed_top.then_some(LogicalPosition::new(previous.x, previous.y)),
        )?)
    };
    if let Some(layout) = &layout {
        apply_layout(layout, &app.state::<GeometryIndex>(), runtime)?;
    }
    let closed_at = current_time_millis()?;
    if let Err(error) = repository.mutate(|store| {
        store
            .notes
            .get_mut(&id)
            .with_context(|| format!("Cannot archive missing note {id}"))?
            .closed_at = Some(closed_at);
        if let Some(layout) = &layout {
            persist_layout_positions(store, layout)?;
        }
        Ok(())
    }) {
        if let Some(layout) = &layout {
            restore_snapshots(&layout.snapshots, &app.state::<GeometryIndex>(), runtime);
        }
        return Err(error.context("Could not archive linked group member"));
    }
    if let Err(close_error) = window.close() {
        let rollback = repository.mutate(|store| {
            *store
                .notes
                .get_mut(&id)
                .with_context(|| format!("Cannot restore missing note {id}"))? = previous.clone();
            if let Some(layout) = &layout {
                for snapshot in &layout.snapshots {
                    let note = store.notes.get_mut(&snapshot.id).with_context(|| {
                        format!("Cannot restore missing group member {}", snapshot.id)
                    })?;
                    let logical = snapshot
                        .position
                        .to_logical::<i32>(snapshot.window.scale_factor()?);
                    note.x = logical.x;
                    note.y = logical.y;
                }
            }
            Ok(())
        });
        if let Some(layout) = &layout {
            restore_snapshots(&layout.snapshots, &app.state::<GeometryIndex>(), runtime);
        }
        rollback.with_context(|| {
            format!("Could not roll back failed window close after: {close_error}")
        })?;
        return Err(close_error.into());
    }
    runtime.drag_origins.remove(&id);
    runtime.completed_drag_origins.remove(&id);
    runtime.programmatic_positions.remove(&id);
    Ok(())
}

pub fn close_window_and_archive(window: &WebviewWindow) -> anyhow::Result<()> {
    let app = window.app_handle();
    let runtime_state = app.state::<GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    let id = note_id_from_label(window.label())?;
    if app.state::<NoteRepository>().group_for_note(id)?.is_some() {
        close_group_member(window, &mut runtime)
    } else {
        close_ungrouped_window_and_archive(window)
    }
}

fn restore_archived_note(
    app: &AppHandle,
    note: &StoredNote,
    runtime: &mut GroupRuntimeState,
) -> anyhow::Result<WebviewWindow> {
    let repository = app.state::<NoteRepository>();
    let window = open_sticky(app, note).context("Could not open archived note")?;
    let restore = (|| -> anyhow::Result<()> {
        let group = repository.group_for_note(&note.id)?;
        let layout = group
            .as_ref()
            .map(|group| {
                let mut active: HashSet<_> = repository
                    .active()?
                    .into_iter()
                    .map(|active_note| active_note.id)
                    .collect();
                active.insert(note.id.clone());
                let ids: Vec<_> = group
                    .note_ids
                    .iter()
                    .filter(|id| active.contains(*id))
                    .cloned()
                    .collect();
                layout_for_ids_at_origin(app, &ids, None).map(Some)
            })
            .transpose()?
            .flatten();
        if let Some(layout) = &layout {
            apply_layout(layout, &app.state::<GeometryIndex>(), runtime)?;
        }
        let persist = repository.mutate(|store| {
            store
                .notes
                .get_mut(&note.id)
                .with_context(|| format!("Cannot restore missing note {}", note.id))?
                .closed_at = None;
            if let Some(layout) = &layout {
                persist_layout_positions(store, layout)?;
            }
            Ok(())
        });
        if let Err(error) = persist {
            if let Some(layout) = &layout {
                restore_snapshots(&layout.snapshots, &app.state::<GeometryIndex>(), runtime);
            }
            return Err(error.context("Could not persist restored group member"));
        }
        Ok(())
    })();
    if let Err(error) = restore {
        let _ = window.close();
        return Err(error);
    }
    Ok(window)
}

pub fn restore_last_closed(app: &AppHandle) -> anyhow::Result<()> {
    let runtime_state = app.state::<GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    let note = app
        .state::<NoteRepository>()
        .last_closed()?
        .context("No recently closed note")?;
    restore_archived_note(app, &note, &mut runtime)?
        .set_focus()
        .context("Could not focus restored note")
}

pub fn restore_all_notes(app: &AppHandle) -> anyhow::Result<()> {
    let runtime_state = app.state::<GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    let repository = app.state::<NoteRepository>();
    let mut archived = repository.archived()?;
    archived.sort_by_key(|note| note.closed_at);
    for note in archived {
        restore_archived_note(app, &note, &mut runtime)?;
    }
    open_missing_active_notes(app)?;
    let windows = sorted_windows(app);
    if windows.is_empty() {
        bail!("No notes to restore");
    }
    for window in &windows {
        window.show()?;
        if window.is_minimized()? {
            window.unminimize()?;
        }
    }
    windows[0]
        .set_focus()
        .context("Could not focus a restored note")
}

fn reset_positions_in_work_area(
    work_area: WindowRect,
    count: usize,
    preferred_step: i32,
    header_height: i32,
) -> anyhow::Result<Vec<PhysicalPosition<i32>>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let margin = i64::from(RESET_MARGIN.max(0));
    let header_height = i64::from(header_height.max(1));
    let x = work_area.x + margin.min(work_area.width.saturating_sub(1).max(0));
    let top = work_area.y + margin.min(work_area.height.saturating_sub(1).max(0));
    let bottom = (work_area.bottom() - header_height - margin).max(top);
    let available = bottom - top;
    let step = if count == 1 {
        0
    } else {
        i64::from(preferred_step.max(0)).min(available / (count as i64 - 1))
    };
    (0..count)
        .map(|index| {
            Ok(PhysicalPosition::new(
                i32::try_from(x).context("Reset x-position exceeded platform limits")?,
                i32::try_from(top + index as i64 * step)
                    .context("Reset y-position exceeded platform limits")?,
            ))
        })
        .collect()
}

pub fn reset_note_positions(app: &AppHandle) -> anyhow::Result<()> {
    let runtime_state = app.state::<GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    open_missing_active_notes(app)?;
    let geometries = app.state::<GeometryIndex>();
    let snapshots = sorted_windows(app)
        .into_iter()
        .map(|window| {
            let id = note_id_from_label(window.label())?.to_string();
            let geometry = geometries.get(&id)?;
            Ok(WindowSnapshot {
                id,
                window,
                position: geometry.position,
                size: geometry.size,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if snapshots.is_empty() {
        return Ok(());
    }
    let monitor = app
        .primary_monitor()?
        .context("No primary monitor available for resetting note positions")?;
    let scale = monitor.scale_factor();
    let work_area =
        WindowRect::from_physical(monitor.work_area().position, monitor.work_area().size);
    let step = (f64::from(COLLAPSED_HEIGHT + GROUP_GAP) * scale).round() as i32;
    let header_height = (f64::from(COLLAPSED_HEIGHT) * scale).round() as i32;
    let targets = reset_positions_in_work_area(work_area, snapshots.len(), step, header_height)?;
    for snapshot in &snapshots {
        snapshot.window.show()?;
        if snapshot.window.is_minimized()? {
            snapshot.window.unminimize()?;
        }
    }
    for (index, (snapshot, target)) in snapshots.iter().zip(&targets).enumerate() {
        if let Err(error) = snapshot.window.set_position(*target) {
            restore_snapshots(&snapshots[..index], &geometries, &mut runtime);
            return Err(error.into());
        }
        geometries.set_position(&snapshot.id, *target)?;
        runtime.record_programmatic_position(snapshot.id.clone(), *target);
    }
    let positions = snapshots
        .iter()
        .zip(&targets)
        .map(|(snapshot, target)| {
            let logical = target.to_logical::<i32>(snapshot.window.scale_factor()?);
            Ok((snapshot.id.clone(), logical.x, logical.y))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let persist = app.state::<NoteRepository>().mutate(|store| {
        store.groups.clear();
        for (id, x, y) in &positions {
            let note = store
                .notes
                .get_mut(id)
                .with_context(|| format!("Cannot reset missing note {id}"))?;
            note.x = *x;
            note.y = *y;
        }
        Ok(())
    });
    if let Err(error) = persist {
        restore_snapshots(&snapshots, &geometries, &mut runtime);
        return Err(error.context("Could not persist reset note positions"));
    }
    snapshots[0]
        .window
        .set_focus()
        .context("Could not focus reset notes")
}

pub fn restore_group_layouts(app: &AppHandle) -> anyhow::Result<()> {
    let runtime_state = app.state::<GroupRuntime>();
    let mut runtime = runtime_state.lock()?;
    let repository = app.state::<NoteRepository>();
    let geometries = app.state::<GeometryIndex>();
    let mut layouts = Vec::new();
    for group in repository.all_groups()? {
        let ids = active_group_ids(&repository, &group, &HashSet::new())?;
        if !ids.is_empty() {
            layouts.push(layout_for_ids_at_origin(app, &ids, None)?);
        }
    }
    if layouts.is_empty() {
        return Ok(());
    }
    for (index, layout) in layouts.iter().enumerate() {
        if let Err(error) = apply_layout(layout, &geometries, &mut runtime) {
            for applied in &layouts[..index] {
                restore_snapshots(&applied.snapshots, &geometries, &mut runtime);
            }
            return Err(error.context("Could not restore linked group layouts"));
        }
    }
    let persist = repository.mutate(|store| {
        for layout in &layouts {
            persist_layout_positions(store, layout)?;
        }
        Ok(())
    });
    if let Err(error) = persist {
        for layout in &layouts {
            restore_snapshots(&layout.snapshots, &geometries, &mut runtime);
        }
        return Err(error.context("Could not persist restored linked group layouts"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(x: i32, y: i32, width: u32, height: u32) -> NoteGeometry {
        NoteGeometry {
            position: PhysicalPosition::new(x, y),
            size: PhysicalSize::new(width, height),
        }
    }

    #[test]
    fn one_click_linking_selects_only_independent_notes_on_the_parent_side() {
        let geometries = GeometryIndex::default();
        for (id, note_geometry) in [
            ("parent", geometry(100, 100, 280, 100)),
            ("top-right", geometry(300, 20, 220, 100)),
            ("bottom", geometry(200, 80, 360, 100)),
            ("top-left", geometry(100, 20, 180, 100)),
            ("other-side", geometry(700, 10, 300, 100)),
            ("other-monitor", geometry(1100, 0, 300, 100)),
        ] {
            geometries.insert(id.into(), note_geometry).unwrap();
        }
        let monitor = WindowRect {
            x: 0,
            y: 0,
            width: 1000,
            height: 800,
        };
        let ids = [
            "top-right",
            "bottom",
            "top-left",
            "other-side",
            "other-monitor",
        ]
        .map(str::to_string);

        assert_eq!(
            ids_on_anchor_monitor_side("parent", &ids, &geometries, monitor, monitor).unwrap(),
            vec!["top-left", "top-right", "bottom"]
        );
    }

    #[test]
    fn restored_mixed_group_uses_durable_heights_and_twelve_pixel_gaps() {
        assert_eq!(
            arranged_positions(LogicalPosition::new(40, 20), &[24, 24, 24, 250, 250, 250],)
                .unwrap(),
            vec![
                LogicalPosition::new(40, 20),
                LogicalPosition::new(40, 56),
                LogicalPosition::new(40, 92),
                LogicalPosition::new(40, 128),
                LogicalPosition::new(40, 390),
                LogicalPosition::new(40, 652),
            ]
        );
    }

    #[test]
    fn height_transition_repositions_only_later_members_from_durable_heights() {
        assert_eq!(
            positions_after_changed_note(LogicalPosition::new(40, 282), 24, &[250, 24]).unwrap(),
            vec![LogicalPosition::new(40, 318), LogicalPosition::new(40, 580),]
        );
    }

    #[test]
    fn delayed_programmatic_move_adopts_native_rounding_instead_of_stale_coordinates() {
        let mut pending = HashMap::from([("note".to_string(), PhysicalPosition::new(100, 200))]);
        let rounded_native_position = PhysicalPosition::new(101, 201);

        assert_eq!(
            position_settlement(
                &mut pending,
                "note",
                rounded_native_position,
                LogicalPosition::new(20, 20),
                1.0,
            ),
            PositionSettlement::AdoptProgrammatic(rounded_native_position)
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn external_move_does_not_consume_the_pending_programmatic_target() {
        let requested = PhysicalPosition::new(100, 200);
        let mut pending = HashMap::from([("note".to_string(), requested)]);

        assert_eq!(
            position_settlement(
                &mut pending,
                "note",
                PhysicalPosition::new(3439, 1354),
                LogicalPosition::new(20, 20),
                1.0,
            ),
            PositionSettlement::ExternalMove
        );
        assert_eq!(pending.get("note"), Some(&requested));
    }

    #[test]
    fn completed_user_drag_waits_for_settled_geometry_before_detachment() {
        let mut runtime = GroupRuntimeState::default();
        let origin = PhysicalPosition::new(100, 100);
        runtime.record_programmatic_position("note".into(), PhysicalPosition::new(90, 90));

        runtime.begin_user_drag("note".into(), origin);
        assert!(!runtime.programmatic_positions.contains_key("note"));
        runtime.complete_user_drag("note").unwrap();

        assert!(!runtime.drag_origins.contains_key("note"));
        let recorded_origin = runtime.take_completed_drag("note").unwrap();
        assert_eq!(recorded_origin, origin);
        assert!(drag_exceeds_threshold(
            recorded_origin.to_logical::<i32>(1.0),
            PhysicalPosition::new(250, 220).to_logical::<i32>(1.0),
        ));
    }

    #[test]
    fn drag_detachment_uses_a_strict_four_pixel_threshold() {
        let start = LogicalPosition::new(100, 100);
        assert!(!drag_exceeds_threshold(
            start,
            LogicalPosition::new(104, 96)
        ));
        assert!(drag_exceeds_threshold(
            start,
            LogicalPosition::new(105, 100)
        ));
    }

    #[test]
    fn dragging_a_group_member_detaches_only_that_note() {
        let mut store: crate::save_load::NoteStore = serde_json::from_value(serde_json::json!({
            "version": 3,
            "notes": {
                "first": {
                    "id": "first", "document": {"type": "doc", "content": []},
                    "color": "#fff9b1", "x": 20, "y": 20,
                    "expanded_height": 250, "expanded_width": 300,
                    "collapsed": false, "pinned": false, "font_size": 16
                },
                "dragged": {
                    "id": "dragged", "document": {"type": "doc", "content": []},
                    "color": "#fff9b1", "x": 20, "y": 282,
                    "expanded_height": 250, "expanded_width": 300,
                    "collapsed": false, "pinned": false, "font_size": 16
                },
                "last": {
                    "id": "last", "document": {"type": "doc", "content": []},
                    "color": "#fff9b1", "x": 20, "y": 544,
                    "expanded_height": 250, "expanded_width": 300,
                    "collapsed": false, "pinned": false, "font_size": 16
                }
            },
            "groups": {
                "group": {"id": "group", "note_ids": ["first", "dragged", "last"]}
            }
        }))
        .unwrap();

        persist_group_detachment(
            &mut store,
            "group",
            "dragged",
            LogicalPosition::new(400, 300),
            LogicalSize::new(300, 250),
        )
        .unwrap();

        assert_eq!((store.notes["first"].x, store.notes["first"].y), (20, 20));
        assert_eq!((store.notes["last"].x, store.notes["last"].y), (20, 544));
        assert_eq!(
            (store.notes["dragged"].x, store.notes["dragged"].y),
            (400, 300)
        );
        assert_eq!(store.groups["group"].note_ids, ["first", "last"]);
    }

    #[test]
    fn reset_positions_keep_every_note_header_in_the_work_area() {
        let work_area = WindowRect {
            x: -1200,
            y: 24,
            width: 900,
            height: 100,
        };
        let positions = reset_positions_in_work_area(work_area, 4, 36, 24).unwrap();

        assert_eq!(positions[0], PhysicalPosition::new(-1180, 44));
        assert_eq!(positions[3], PhysicalPosition::new(-1180, 80));
        assert!(positions.iter().all(|position| {
            i64::from(position.y) >= work_area.y && i64::from(position.y) + 24 <= work_area.bottom()
        }));
    }
}
