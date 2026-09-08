use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "macos")]
use std::{cell::RefCell, time::Instant};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, EventTarget, LogicalPosition, LogicalSize, Manager, PhysicalPosition,
    PhysicalSize, WebviewWindow, WindowEvent, Wry,
};
use tauri_plugin_log::log;
use tauri_plugin_store::{Store, StoreExt};
use uuid::Uuid;

use crate::{
    pinned_windows::sync_pinned_window_registry,
    windows::{apply_window_pin_state, GeometryIndex, NoteGeometry},
};

const TIMER_STORE_FILE: &str = "timers.json";
const TIMER_STORE_VERSION: u32 = 2;
pub(crate) const TIMER_WIDTH: u32 = 300;
pub(crate) const TIMER_HEIGHT: u32 = 176;
pub(crate) const TIMER_COLLAPSED_HEIGHT: u32 = 24;
const WINDOW_GAP: i64 = 20;
const MILLIS_PER_SECOND: u64 = 1_000;
const MILLIS_PER_MINUTE: u64 = 60_000;
const MILLIS_PER_HOUR: u64 = 60 * MILLIS_PER_MINUTE;
#[cfg(target_os = "macos")]
const ALARM_GROUP_COUNT: usize = 7;
#[cfg(target_os = "macos")]
const ALARM_PLAYS_PER_GROUP: usize = 3;
#[cfg(target_os = "macos")]
const ALARM_INTRA_GROUP_MS: u64 = 350;
#[cfg(target_os = "macos")]
const ALARM_INTER_GROUP_SILENCE_MS: u64 = 900;
#[cfg(target_os = "macos")]
static ALARM_PLAYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "macos")]
static ALARM_SOUND_DURATION_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(target_os = "macos")]
struct AlarmSoundPool {
    sounds: Vec<objc2::rc::Retained<objc2_app_kit::NSSound>>,
    next: usize,
}

#[cfg(target_os = "macos")]
thread_local! {
    static ALARM_SOUNDS: RefCell<Option<AlarmSoundPool>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StoredTimer {
    pub id: String,
    pub accumulated_ms: u64,
    pub running_since_ms: Option<u64>,
    pub reminder_interval_ms: u64,
    #[serde(default)]
    pub alarm_at_ms: u64,
    pub pinned: bool,
    pub x: i32,
    pub y: i32,
    #[serde(default)]
    pub collapsed: bool,
}

impl StoredTimer {
    fn new(x: i32, y: i32) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            accumulated_ms: 0,
            running_since_ms: None,
            reminder_interval_ms: 0,
            alarm_at_ms: 0,
            pinned: false,
            x,
            y,
            collapsed: false,
        }
    }

    fn elapsed_at(&self, now_ms: u64) -> u64 {
        self.accumulated_ms.saturating_add(
            self.running_since_ms
                .map(|started| now_ms.saturating_sub(started))
                .unwrap_or(0),
        )
    }

    fn pause_at(&mut self, now_ms: u64) {
        self.accumulated_ms = self.elapsed_at(now_ms);
        self.running_since_ms = None;
    }

    fn resume_at(&mut self, now_ms: u64) {
        if self.running_since_ms.is_none() {
            self.running_since_ms = Some(now_ms);
        }
    }

    fn reset(&mut self) {
        self.accumulated_ms = 0;
        self.running_since_ms = None;
    }

    fn set_elapsed(&mut self, hours: u64, minutes: u8, seconds: u8) -> anyhow::Result<()> {
        if self.running_since_ms.is_some() {
            bail!("Elapsed time can only be edited while the timer is paused");
        }
        if minutes > 59 || seconds > 59 {
            bail!("Elapsed minutes and seconds must be between 0 and 59");
        }
        self.accumulated_ms = duration_from_parts(hours, minutes, seconds)?;
        Ok(())
    }

    fn set_sound_settings(
        &mut self,
        reminder_hours: u64,
        reminder_minutes: u8,
        reminder_seconds: u8,
        alarm_hours: u64,
        alarm_minutes: u8,
        alarm_seconds: u8,
    ) -> anyhow::Result<()> {
        if reminder_minutes > 59 || reminder_seconds > 59 {
            bail!("Reminder minutes and seconds must be between 0 and 59");
        }
        if alarm_minutes > 59 || alarm_seconds > 59 {
            bail!("Alarm minutes and seconds must be between 0 and 59");
        }
        let reminder_interval_ms =
            duration_from_parts(reminder_hours, reminder_minutes, reminder_seconds)?;
        let alarm_at_ms = duration_from_parts(alarm_hours, alarm_minutes, alarm_seconds)?;
        self.reminder_interval_ms = reminder_interval_ms;
        self.alarm_at_ms = alarm_at_ms;
        Ok(())
    }
}

fn duration_from_parts(hours: u64, minutes: u8, seconds: u8) -> anyhow::Result<u64> {
    hours
        .checked_mul(MILLIS_PER_HOUR)
        .and_then(|hours_ms| {
            u64::from(minutes)
                .checked_mul(MILLIS_PER_MINUTE)
                .and_then(|minutes_ms| hours_ms.checked_add(minutes_ms))
        })
        .and_then(|duration_ms| {
            u64::from(seconds)
                .checked_mul(MILLIS_PER_SECOND)
                .and_then(|seconds_ms| duration_ms.checked_add(seconds_ms))
        })
        .context("Timer duration is too large")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PersistedTimerStore {
    version: u32,
    timers: BTreeMap<String, StoredTimer>,
}

impl PersistedTimerStore {
    fn empty() -> Self {
        Self {
            version: TIMER_STORE_VERSION,
            timers: BTreeMap::new(),
        }
    }
}

pub struct TimerRepository {
    store: Option<Arc<Store<Wry>>>,
    timers: Mutex<BTreeMap<String, StoredTimer>>,
    unavailable_reason: Option<String>,
}

impl TimerRepository {
    pub fn load(app: &AppHandle) -> anyhow::Result<Self> {
        let app_data_dir = app
            .path()
            .app_data_dir()
            .context("Could not locate Sticky application-data directory")?;
        fs::create_dir_all(&app_data_dir)
            .context("Could not create Sticky application-data directory")?;
        let path = app_data_dir.join(TIMER_STORE_FILE);

        let persisted = if path.exists() {
            match read_and_validate_store(&path) {
                Ok(store) => store,
                Err(error) => {
                    let quarantine = quarantine_bad_store(&app_data_dir, &path)?;
                    log::error!(
                        "Timer store is unreadable ({error:#}); quarantined it at {:?}",
                        quarantine
                    );
                    PersistedTimerStore::empty()
                }
            }
        } else {
            PersistedTimerStore::empty()
        };

        let store = app
            .store_builder(TIMER_STORE_FILE)
            .disable_auto_save()
            .build()
            .context("Could not open timer storage")?;
        let repository = Self {
            store: Some(store),
            timers: Mutex::new(persisted.timers),
            unavailable_reason: None,
        };
        repository.persist_current()?;
        Ok(repository)
    }

    pub fn unavailable(error: anyhow::Error) -> Self {
        Self {
            store: None,
            timers: Mutex::new(BTreeMap::new()),
            unavailable_reason: Some(format!("{error:#}")),
        }
    }

    pub fn all(&self) -> anyhow::Result<Vec<StoredTimer>> {
        self.ensure_available()?;
        Ok(self
            .timers
            .lock()
            .map_err(|_| anyhow::anyhow!("Timer storage lock poisoned"))?
            .values()
            .cloned()
            .collect())
    }

    pub fn is_available(&self) -> bool {
        self.unavailable_reason.is_none()
    }

    pub fn get(&self, id: &str) -> anyhow::Result<StoredTimer> {
        self.ensure_available()?;
        self.timers
            .lock()
            .map_err(|_| anyhow::anyhow!("Timer storage lock poisoned"))?
            .get(id)
            .cloned()
            .with_context(|| format!("Unknown timer id {id}"))
    }

    fn create(&self, x: i32, y: i32) -> anyhow::Result<StoredTimer> {
        let timer = StoredTimer::new(x, y);
        let result = timer.clone();
        self.mutate(|timers| {
            timers.insert(timer.id.clone(), timer);
            Ok(())
        })?;
        Ok(result)
    }

    pub(crate) fn insert(&self, timer: StoredTimer) -> anyhow::Result<()> {
        self.mutate(|timers| {
            timers.insert(timer.id.clone(), timer);
            Ok(())
        })
    }

    pub(crate) fn update<F>(&self, id: &str, update: F) -> anyhow::Result<StoredTimer>
    where
        F: FnOnce(&mut StoredTimer) -> anyhow::Result<()>,
    {
        let mut result = None;
        self.mutate(|timers| {
            let timer = timers
                .get_mut(id)
                .with_context(|| format!("Unknown timer id {id}"))?;
            update(timer)?;
            result = Some(timer.clone());
            Ok(())
        })?;
        result.context("Timer update did not produce a result")
    }

    pub(crate) fn delete(&self, id: &str) -> anyhow::Result<StoredTimer> {
        let mut removed = None;
        self.mutate(|timers| {
            removed = timers.remove(id);
            if removed.is_none() {
                bail!("Unknown timer id {id}");
            }
            Ok(())
        })?;
        removed.context("Timer deletion did not produce a result")
    }

    pub(crate) fn mutate<F>(&self, update: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut BTreeMap<String, StoredTimer>) -> anyhow::Result<()>,
    {
        self.ensure_available()?;
        let mut guard = self
            .timers
            .lock()
            .map_err(|_| anyhow::anyhow!("Timer storage lock poisoned"))?;
        let mut candidate = guard.clone();
        update(&mut candidate)?;
        validate_timers(&candidate)?;
        self.persist(&candidate)?;
        *guard = candidate;
        Ok(())
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let timers = self
            .timers
            .lock()
            .map_err(|_| anyhow::anyhow!("Timer storage lock poisoned"))?;
        self.persist(&timers)
    }

    fn persist(&self, timers: &BTreeMap<String, StoredTimer>) -> anyhow::Result<()> {
        let store = self
            .store
            .as_ref()
            .context("Timer storage is unavailable")?;
        store.set("version", TIMER_STORE_VERSION);
        store.set("timers", serde_json::to_value(timers)?);
        store.save().context("Could not save timer storage")
    }

    fn ensure_available(&self) -> anyhow::Result<()> {
        if let Some(reason) = &self.unavailable_reason {
            bail!("Timer storage is unavailable: {reason}");
        }
        Ok(())
    }
}

fn read_and_validate_store(path: &Path) -> anyhow::Result<PersistedTimerStore> {
    let bytes = fs::read(path).context("Could not read timer storage")?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&bytes).context("Could not parse timer storage")?;
    if value.get("version").and_then(serde_json::Value::as_u64) == Some(1) {
        let object = value
            .as_object_mut()
            .context("Timer storage root was not an object")?;
        object.insert("version".into(), TIMER_STORE_VERSION.into());
    }
    let store: PersistedTimerStore =
        serde_json::from_value(value).context("Could not parse timer storage")?;
    if store.version != TIMER_STORE_VERSION {
        bail!(
            "Unsupported timer storage version {} (expected {})",
            store.version,
            TIMER_STORE_VERSION
        );
    }
    validate_timers(&store.timers)?;
    Ok(store)
}

fn validate_timers(timers: &BTreeMap<String, StoredTimer>) -> anyhow::Result<()> {
    for (key, timer) in timers {
        if key != &timer.id || timer.id.is_empty() {
            bail!("Timer map key did not match the stored timer id");
        }
    }
    Ok(())
}

fn quarantine_bad_store(app_data_dir: &Path, path: &Path) -> anyhow::Result<PathBuf> {
    let backup_dir = app_data_dir.join("backups");
    fs::create_dir_all(&backup_dir).context("Could not create timer backup directory")?;
    let timestamp = current_time_millis()?;
    let destination = backup_dir.join(format!(
        "corrupt-{TIMER_STORE_FILE}-{timestamp}-{}.json",
        Uuid::new_v4()
    ));
    fs::rename(path, &destination).context("Could not quarantine unreadable timer storage")?;
    Ok(destination)
}

fn current_time_millis() -> anyhow::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System time is before UNIX epoch")?
        .as_millis()
        .try_into()
        .context("System timestamp did not fit in timer storage")
}

#[derive(Debug, Clone, Serialize)]
pub struct TimerSnapshot {
    id: String,
    elapsed_ms: u64,
    running: bool,
    reminder_interval_ms: u64,
    alarm_at_ms: u64,
    always_on_top: bool,
    collapsed: bool,
}

impl TimerSnapshot {
    fn at(timer: &StoredTimer, now_ms: u64) -> Self {
        Self {
            id: timer.id.clone(),
            elapsed_ms: timer.elapsed_at(now_ms),
            running: timer.running_since_ms.is_some(),
            reminder_interval_ms: timer.reminder_interval_ms,
            alarm_at_ms: timer.alarm_at_ms,
            always_on_top: timer.pinned,
            collapsed: timer.collapsed,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
struct TimerTick {
    elapsed_ms: u64,
    running: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ReminderSchedule {
    interval_ms: u64,
    next_boundary_ms: Option<u64>,
}

impl ReminderSchedule {
    fn new(elapsed_ms: u64, interval_ms: u64) -> Self {
        Self {
            interval_ms,
            next_boundary_ms: next_future_multiple(elapsed_ms, interval_ms),
        }
    }

    fn crossed_boundary(&mut self, elapsed_ms: u64, interval_ms: u64) -> bool {
        if self.interval_ms != interval_ms {
            *self = Self::new(elapsed_ms, interval_ms);
            return false;
        }
        let crossed = self
            .next_boundary_ms
            .is_some_and(|boundary| elapsed_ms >= boundary);
        if crossed {
            self.next_boundary_ms = next_future_multiple(elapsed_ms, interval_ms);
        }
        crossed
    }
}

fn next_future_multiple(elapsed_ms: u64, interval_ms: u64) -> Option<u64> {
    if interval_ms == 0 {
        return None;
    }
    elapsed_ms
        .checked_div(interval_ms)?
        .checked_add(1)?
        .checked_mul(interval_ms)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AlarmSchedule {
    alarm_at_ms: u64,
    pending_ms: Option<u64>,
}

impl AlarmSchedule {
    fn new(elapsed_ms: u64, alarm_at_ms: u64) -> Self {
        Self {
            alarm_at_ms,
            pending_ms: (alarm_at_ms > elapsed_ms && alarm_at_ms > 0).then_some(alarm_at_ms),
        }
    }

    fn crossed_boundary(&mut self, elapsed_ms: u64, alarm_at_ms: u64) -> bool {
        if self.alarm_at_ms != alarm_at_ms {
            *self = Self::new(elapsed_ms, alarm_at_ms);
            return false;
        }
        let crossed = self
            .pending_ms
            .is_some_and(|boundary| elapsed_ms >= boundary);
        if crossed {
            self.pending_ms = None;
        }
        crossed
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TimerSchedule {
    reminder: ReminderSchedule,
    alarm: AlarmSchedule,
}

impl TimerSchedule {
    fn new(timer: &StoredTimer, elapsed_ms: u64) -> Self {
        Self {
            reminder: ReminderSchedule::new(elapsed_ms, timer.reminder_interval_ms),
            alarm: AlarmSchedule::new(elapsed_ms, timer.alarm_at_ms),
        }
    }
}

#[derive(Default)]
pub struct TimerRuntime(Mutex<HashMap<String, TimerSchedule>>);

impl TimerRuntime {
    fn reset_for(&self, timer: &StoredTimer, now_ms: u64) -> anyhow::Result<()> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("Timer scheduler lock poisoned"))?
            .insert(
                timer.id.clone(),
                TimerSchedule::new(timer, timer.elapsed_at(now_ms)),
            );
        Ok(())
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

fn is_surface_label(label: &str) -> bool {
    label.starts_with("sticky_") || label.starts_with("timer_")
}

fn get_position_and_size(
    window: &WebviewWindow,
) -> anyhow::Result<(PhysicalPosition<i32>, PhysicalSize<u32>)> {
    Ok((window.outer_position()?, window.outer_size()?))
}

fn find_new_timer_position(app: &AppHandle) -> anyhow::Result<LogicalPosition<i32>> {
    let surfaces: Vec<_> = app
        .webview_windows()
        .into_iter()
        .filter(|(label, window)| is_surface_label(label) && window.is_visible().unwrap_or(false))
        .map(|(_, window)| window)
        .collect();
    let anchor = surfaces
        .iter()
        .find(|window| window.is_focused().unwrap_or(false))
        .cloned()
        .or_else(|| surfaces.last().cloned());
    let monitor = if let Some(anchor) = &anchor {
        anchor
            .current_monitor()?
            .or(anchor.primary_monitor()?)
            .context("No monitor is available for placing a timer")?
    } else {
        app.primary_monitor()?
            .context("No monitor is available for placing a timer")?
    };
    let scale_factor = monitor.scale_factor();
    let work_area =
        WindowRect::from_physical(monitor.work_area().position, monitor.work_area().size);
    let new_size = LogicalSize::new(TIMER_WIDTH, TIMER_HEIGHT).to_physical(scale_factor);
    let anchor_rect = anchor
        .as_ref()
        .and_then(|window| get_position_and_size(window).ok())
        .map(|(position, size)| WindowRect::from_physical(position, size))
        .unwrap_or(WindowRect {
            x: work_area.x + WINDOW_GAP,
            y: work_area.y + WINDOW_GAP,
            width: 0,
            height: 0,
        });
    let obstacles: Vec<_> = surfaces
        .iter()
        .filter(|window| {
            window
                .current_monitor()
                .ok()
                .flatten()
                .is_some_and(|candidate| candidate.name() == monitor.name())
        })
        .filter_map(|window| get_position_and_size(window).ok())
        .map(|(position, size)| WindowRect::from_physical(position, size))
        .collect();

    nearest_free_position(anchor_rect, &obstacles, work_area, new_size)
        .map(|position| position.to_logical(scale_factor))
        .context("No non-overlapping space is available for a new timer")
}

fn nearest_free_position(
    anchor: WindowRect,
    obstacles: &[WindowRect],
    work_area: WindowRect,
    new_size: PhysicalSize<u32>,
) -> Option<PhysicalPosition<i32>> {
    let width = i64::from(new_size.width);
    let height = i64::from(new_size.height);
    let gap = WINDOW_GAP;
    let mut positions = vec![
        (anchor.right() + gap, anchor.y),
        (anchor.x, anchor.bottom() + gap),
        (anchor.x - width - gap, anchor.y),
        (anchor.x, anchor.y - height - gap),
    ];
    let mut xs = vec![work_area.x + gap, work_area.right() - width - gap];
    let mut ys = vec![work_area.y + gap, work_area.bottom() - height - gap];
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

pub fn timer_id_from_label(label: &str) -> anyhow::Result<&str> {
    label
        .strip_prefix("timer_")
        .context("Window label did not contain a timer id")
}

pub fn timer_registry_identity(id: &str) -> String {
    format!("timer:{id}")
}

pub fn create_timer_window(app: &AppHandle) -> anyhow::Result<WebviewWindow> {
    let position = find_new_timer_position(app)?;
    let repository = app.state::<TimerRepository>();
    let timer = repository.create(position.x, position.y)?;
    match open_timer_window(app, &timer, true) {
        Ok(window) => Ok(window),
        Err(open_error) => {
            repository.delete(&timer.id).with_context(|| {
                format!("Could not roll back failed timer creation after: {open_error:#}")
            })?;
            Err(open_error.context("Could not open the newly created timer"))
        }
    }
}

fn open_timer_window(
    app: &AppHandle,
    timer: &StoredTimer,
    focus: bool,
) -> anyhow::Result<WebviewWindow> {
    let now_ms = current_time_millis()?;
    let init = TimerSnapshot::at(timer, now_ms);
    let init_script = format!("window.__TIMER_INIT__ = {};", serde_json::to_string(&init)?);
    let label = format!("timer_{}", timer.id);
    let window =
        tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App("index.html".into()))
            .title("Timer")
            .decorations(false)
            .maximizable(false)
            .resizable(false)
            .visible(true)
            .focused(focus)
            .accept_first_mouse(true)
            .initialization_script(init_script)
            .inner_size(
                f64::from(TIMER_WIDTH),
                f64::from(if timer.collapsed {
                    TIMER_COLLAPSED_HEIGHT
                } else {
                    TIMER_HEIGHT
                }),
            )
            .position(f64::from(timer.x), f64::from(timer.y))
            .always_on_top(timer.pinned)
            .prevent_overflow()
            .build()
            .context("Could not create timer window")?;

    app.state::<GeometryIndex>().insert(
        timer.id.clone(),
        NoteGeometry {
            position: window.outer_position()?,
            size: window.outer_size()?,
        },
    )?;

    let actual_position = window
        .outer_position()?
        .to_logical::<i32>(window.scale_factor()?);
    if actual_position.x != timer.x || actual_position.y != timer.y {
        app.state::<TimerRepository>().update(&timer.id, |stored| {
            stored.x = actual_position.x;
            stored.y = actual_position.y;
            Ok(())
        })?;
    }

    let app_clone = app.clone();
    let timer_id = timer.id.clone();
    let identity = timer_registry_identity(&timer.id);
    window.on_window_event(move |event| {
        if let Err(error) = app_clone
            .state::<GeometryIndex>()
            .record_window_event(&timer_id, event)
        {
            log::error!("Could not update live geometry for timer {timer_id}: {error:#}");
        }
        if matches!(event, WindowEvent::Destroyed) {
            if let Err(error) = sync_pinned_window_registry(&app_clone, Some(&identity)) {
                log::error!(
                    "Could not remove destroyed timer from pinned-window registry: {error:#}"
                );
            }
        }
    });

    apply_window_pin_state(&window, timer.pinned)?;
    app.state::<TimerRuntime>().reset_for(timer, now_ms)?;
    if let Err(error) = sync_pinned_window_registry(app, None) {
        log::error!(
            "Could not register pin state for opened timer {}: {error:#}",
            timer.id
        );
    }
    if focus {
        window.set_focus()?;
    }
    Ok(window)
}

pub fn restore_timer_windows(app: &AppHandle) -> anyhow::Result<()> {
    let previously_focused = app
        .webview_windows()
        .into_values()
        .find(|window| window.is_focused().unwrap_or(false));
    for timer in app.state::<TimerRepository>().all()? {
        if let Err(error) = open_timer_window(app, &timer, false) {
            log::error!("Could not restore timer {}: {error:#}", timer.id);
        }
    }
    if let Some(window) = previously_focused {
        window.set_focus()?;
    }
    Ok(())
}

pub(crate) fn set_ungrouped_timer_collapsed(
    window: &WebviewWindow,
    collapsed: bool,
) -> anyhow::Result<()> {
    let id = timer_id_from_label(window.label())?;
    let repository = window.state::<TimerRepository>();
    let previous = repository.get(id)?;
    if previous.collapsed == collapsed {
        return Ok(());
    }
    let previous_size = window.outer_size()?;
    let target = LogicalSize::new(
        TIMER_WIDTH,
        if collapsed {
            TIMER_COLLAPSED_HEIGHT
        } else {
            TIMER_HEIGHT
        },
    );
    window.set_size(target)?;
    let physical = window.outer_size()?;
    window.state::<GeometryIndex>().set_size(id, physical)?;
    if let Err(error) = repository.update(id, |timer| {
        timer.collapsed = collapsed;
        Ok(())
    }) {
        let _ = window.set_size(previous_size);
        let _ = window.state::<GeometryIndex>().set_size(id, previous_size);
        return Err(error.context("Could not persist timer folded state"));
    }
    if !collapsed {
        window.set_focus()?;
    }
    Ok(())
}

pub fn start_timer_worker(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        if let Err(error) = timer_worker_tick(&app) {
            log::error!("Timer worker tick failed: {error:#}");
        }
    });
}

fn timer_worker_tick(app: &AppHandle) -> anyhow::Result<()> {
    let now_ms = current_time_millis()?;
    let repository = app.state::<TimerRepository>();
    let runtime = app.state::<TimerRuntime>();
    let mut schedules = runtime
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("Timer scheduler lock poisoned"))?;
    let timers = repository.all()?;
    let known_ids: HashSet<_> = timers.iter().map(|timer| timer.id.as_str()).collect();
    schedules.retain(|id, _| known_ids.contains(id.as_str()));

    let mut should_beep = false;
    let mut should_alarm = false;
    for timer in timers {
        let elapsed_ms = timer.elapsed_at(now_ms);
        let running = timer.running_since_ms.is_some();
        let schedule = schedules
            .entry(timer.id.clone())
            .or_insert_with(|| TimerSchedule::new(&timer, elapsed_ms));
        if running {
            if schedule
                .reminder
                .crossed_boundary(elapsed_ms, timer.reminder_interval_ms)
            {
                should_beep = true;
            }
            if schedule
                .alarm
                .crossed_boundary(elapsed_ms, timer.alarm_at_ms)
            {
                should_alarm = true;
            }
        }
        let _ = app.emit_to(
            EventTarget::webview_window(format!("timer_{}", timer.id)),
            "timer_tick",
            TimerTick {
                elapsed_ms,
                running,
            },
        );
    }
    drop(schedules);

    #[cfg(target_os = "macos")]
    {
        if should_alarm {
            start_timer_alarm(app.clone());
        }
        if should_beep {
            app.run_on_main_thread(sound_timer_beep)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn sound_timer_beep() {
    // The macOS alert displayed as “Pong” uses the legacy Morse identifier.
    play_named_sound("Morse", "recurring timer beep");
}

#[cfg(target_os = "macos")]
pub fn initialize_alarm_sounds(app: &AppHandle) -> anyhow::Result<()> {
    use objc2::AnyThread;
    use objc2_app_kit::NSSound;
    use objc2_foundation::NSString;

    let path = app
        .path()
        .resource_dir()
        .context("Could not locate Sticky's bundled resources")?
        .join("codex-notification.wav");
    if !path.is_file() {
        bail!(
            "Bundled timer alarm asset is missing at {}; reinstall Sticky from a complete build",
            path.display()
        );
    }
    let path_string = NSString::from_str(
        path.to_str()
            .context("Bundled timer alarm asset path was not valid Unicode")?,
    );
    let mut sounds = Vec::with_capacity(3);
    for index in 0..3 {
        let sound =
            NSSound::initWithContentsOfFile_byReference(NSSound::alloc(), &path_string, false)
                .with_context(|| {
                    format!(
                        "Could not preload timer alarm sound instance {} from {}",
                        index + 1,
                        path.display()
                    )
                })?;
        sounds.push(sound);
    }
    let duration_ms = sounds[0].duration();
    if !duration_ms.is_finite() || duration_ms <= 0.0 {
        bail!("Bundled timer alarm asset reported an invalid duration");
    }
    ALARM_SOUND_DURATION_MS.store(
        (duration_ms * 1_000.0).ceil() as u64,
        std::sync::atomic::Ordering::Release,
    );
    ALARM_SOUNDS.with(|pool| {
        *pool.borrow_mut() = Some(AlarmSoundPool { sounds, next: 0 });
    });
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn initialize_alarm_sounds(_app: &AppHandle) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn sound_timer_alarm() {
    ALARM_SOUNDS.with(|pool| {
        let mut pool = pool.borrow_mut();
        let Some(pool) = pool.as_mut() else {
            log::error!(
                "Timer alarm cannot play because codex-notification.wav was not preloaded; reinstall Sticky from a complete build"
            );
            return;
        };
        let index = pool.next;
        pool.next = (pool.next + 1) % pool.sounds.len();
        if !pool.sounds[index].play() {
            log::error!(
                "macOS refused timer alarm playback on preloaded sound instance {}",
                index + 1
            );
        }
    });
}

#[cfg(target_os = "macos")]
fn alarm_playback_starts(sound_duration_ms: u64) -> Vec<u64> {
    let mut starts = Vec::with_capacity(ALARM_GROUP_COUNT * ALARM_PLAYS_PER_GROUP);
    let mut group_start = 0;
    for group in 0..ALARM_GROUP_COUNT {
        for playback in 0..ALARM_PLAYS_PER_GROUP {
            starts.push(group_start + playback as u64 * ALARM_INTRA_GROUP_MS);
        }
        if group + 1 < ALARM_GROUP_COUNT {
            group_start += (ALARM_PLAYS_PER_GROUP as u64 - 1) * ALARM_INTRA_GROUP_MS
                + sound_duration_ms
                + ALARM_INTER_GROUP_SILENCE_MS;
        }
    }
    starts
}

#[cfg(target_os = "macos")]
fn start_timer_alarm(app: AppHandle) {
    use std::sync::atomic::Ordering;

    if ALARM_PLAYING.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::spawn(move || {
        let sound_duration_ms = ALARM_SOUND_DURATION_MS.load(Ordering::Acquire);
        if sound_duration_ms == 0 {
            log::error!(
                "Timer alarm cannot start because codex-notification.wav was not preloaded"
            );
            ALARM_PLAYING.store(false, Ordering::Release);
            return;
        }
        let started = Instant::now();
        let starts = alarm_playback_starts(sound_duration_ms);
        for offset_ms in &starts {
            let target = Duration::from_millis(*offset_ms);
            if let Some(remaining) = target.checked_sub(started.elapsed()) {
                thread::sleep(remaining);
            }
            if let Err(error) = app.run_on_main_thread(sound_timer_alarm) {
                log::error!("Could not schedule timer alarm sound: {error}");
                break;
            }
        }
        if let Some(last_start) = starts.last() {
            let target = Duration::from_millis(last_start + sound_duration_ms);
            if let Some(remaining) = target.checked_sub(started.elapsed()) {
                thread::sleep(remaining);
            }
        }
        ALARM_PLAYING.store(false, Ordering::Release);
    });
}

#[cfg(target_os = "macos")]
fn play_named_sound(name: &str, description: &str) {
    use objc2_app_kit::NSSound;
    use objc2_foundation::NSString;

    let name = NSString::from_str(name);
    let Some(sound) = NSSound::soundNamed(&name) else {
        log::error!("Could not load the macOS sound for {description}");
        return;
    };
    if !sound.play() {
        log::error!("macOS refused to start playback for {description}");
    }
}

fn update_timer_state<F>(window: &WebviewWindow, update: F) -> Result<TimerSnapshot, String>
where
    F: FnOnce(&mut StoredTimer, u64) -> anyhow::Result<()>,
{
    let id = timer_id_from_label(window.label()).map_err(|error| error.to_string())?;
    let now_ms = current_time_millis().map_err(|error| error.to_string())?;
    let runtime = window.state::<TimerRuntime>();
    let mut schedules = runtime
        .0
        .lock()
        .map_err(|_| "Timer scheduler lock poisoned".to_string())?;
    let timer = window
        .state::<TimerRepository>()
        .update(id, |timer| update(timer, now_ms))
        .map_err(|error| error.to_string())?;
    schedules.insert(
        id.to_string(),
        TimerSchedule::new(&timer, timer.elapsed_at(now_ms)),
    );
    Ok(TimerSnapshot::at(&timer, now_ms))
}

#[tauri::command]
pub fn timer_pause(window: WebviewWindow) -> Result<TimerSnapshot, String> {
    update_timer_state(&window, |timer, now_ms| {
        timer.pause_at(now_ms);
        Ok(())
    })
}

#[tauri::command]
pub fn timer_resume(window: WebviewWindow) -> Result<TimerSnapshot, String> {
    update_timer_state(&window, |timer, now_ms| {
        timer.resume_at(now_ms);
        Ok(())
    })
}

#[tauri::command]
pub fn timer_reset(window: WebviewWindow) -> Result<TimerSnapshot, String> {
    update_timer_state(&window, |timer, _now_ms| {
        timer.reset();
        Ok(())
    })
}

#[tauri::command]
pub fn timer_set_elapsed(
    window: WebviewWindow,
    hours: u64,
    minutes: u8,
    seconds: u8,
) -> Result<TimerSnapshot, String> {
    update_timer_state(&window, |timer, _now_ms| {
        timer.set_elapsed(hours, minutes, seconds)
    })
}

#[tauri::command]
pub fn timer_apply_settings(
    window: WebviewWindow,
    reminder_hours: u64,
    reminder_minutes: u8,
    reminder_seconds: u8,
    alarm_hours: u64,
    alarm_minutes: u8,
    alarm_seconds: u8,
) -> Result<TimerSnapshot, String> {
    update_timer_state(&window, |timer, _now_ms| {
        timer.set_sound_settings(
            reminder_hours,
            reminder_minutes,
            reminder_seconds,
            alarm_hours,
            alarm_minutes,
            alarm_seconds,
        )
    })
}

#[tauri::command]
pub fn set_timer_always_on_top(window: WebviewWindow, always_on_top: bool) -> Result<(), String> {
    let id = timer_id_from_label(window.label()).map_err(|error| error.to_string())?;
    let repository = window.state::<TimerRepository>();
    let previous = repository
        .get(id)
        .map_err(|error| error.to_string())?
        .pinned;
    apply_window_pin_state(&window, always_on_top).map_err(|error| error.to_string())?;
    if let Err(error) = repository.update(id, |timer| {
        timer.pinned = always_on_top;
        Ok(())
    }) {
        let _ = apply_window_pin_state(&window, previous);
        return Err(error.to_string());
    }
    if let Err(error) = sync_pinned_window_registry(window.app_handle(), None) {
        repository
            .update(id, |timer| {
                timer.pinned = previous;
                Ok(())
            })
            .map_err(|rollback| rollback.to_string())?;
        apply_window_pin_state(&window, previous).map_err(|rollback| rollback.to_string())?;
        return Err(error.to_string());
    }
    window.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn close_timer(window: WebviewWindow) -> Result<(), String> {
    let removed = remove_timer_for_close(&window).map_err(|error| error.to_string())?;
    if let Err(error) = window.close() {
        restore_timer_after_failed_close(&window, removed).map_err(|rollback| {
            format!("Could not restore timer after failed window close: {rollback:#}")
        })?;
        return Err(error.to_string());
    }
    Ok(())
}

pub(crate) fn remove_timer_for_close(window: &WebviewWindow) -> anyhow::Result<StoredTimer> {
    let id = timer_id_from_label(window.label())?.to_string();
    let runtime = window.state::<TimerRuntime>();
    let mut schedules = runtime
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("Timer scheduler lock poisoned"))?;
    let repository = window.state::<TimerRepository>();
    let removed = repository.delete(&id)?;
    schedules.remove(&id);
    Ok(removed)
}

pub(crate) fn restore_timer_after_failed_close(
    window: &WebviewWindow,
    timer: StoredTimer,
) -> anyhow::Result<()> {
    let id = timer.id.clone();
    window.state::<TimerRepository>().insert(timer.clone())?;
    window
        .state::<TimerRuntime>()
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("Timer scheduler lock poisoned"))?
        .insert(
            id,
            TimerSchedule::new(&timer, timer.elapsed_at(current_time_millis().unwrap_or(0))),
        );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timer(accumulated_ms: u64, running_since_ms: Option<u64>) -> StoredTimer {
        StoredTimer {
            id: "timer".into(),
            accumulated_ms,
            running_since_ms,
            reminder_interval_ms: 0,
            alarm_at_ms: 0,
            pinned: false,
            x: 0,
            y: 0,
            collapsed: false,
        }
    }

    #[test]
    fn elapsed_time_survives_pause_resume_and_relaunch() {
        let mut timer = timer(2_000, Some(10_000));
        assert_eq!(timer.elapsed_at(15_000), 7_000);
        timer.pause_at(15_000);
        assert_eq!(timer.elapsed_at(30_000), 7_000);
        timer.resume_at(40_000);
        let relaunched = timer.clone();
        assert_eq!(relaunched.elapsed_at(45_500), 12_500);
    }

    #[test]
    fn paused_elapsed_edits_reset_and_sound_settings_preserve_their_clock_contract() {
        let mut timer = timer(3_723_456, None);
        timer.set_elapsed(2, 5, 7).unwrap();
        assert_eq!(timer.accumulated_ms, 7_507_000);
        timer.set_sound_settings(1, 30, 15, 2, 15, 30).unwrap();
        assert_eq!(timer.accumulated_ms, 7_507_000);
        assert_eq!(timer.reminder_interval_ms, 5_415_000);
        assert_eq!(timer.alarm_at_ms, 8_130_000);
        timer.resume_at(10_000);
        assert!(timer.set_elapsed(0, 0, 0).is_err());
        timer.reset();
        assert_eq!(timer.elapsed_at(99_000), 0);
        assert!(timer.running_since_ms.is_none());
    }

    #[test]
    fn reminders_repeat_at_elapsed_boundaries_without_duplicates() {
        let mut schedule = ReminderSchedule::new(9_000, 5_000);
        assert!(!schedule.crossed_boundary(9_999, 5_000));
        assert!(schedule.crossed_boundary(10_000, 5_000));
        assert!(!schedule.crossed_boundary(10_000, 5_000));
        assert!(schedule.crossed_boundary(15_100, 5_000));
    }

    #[test]
    fn sleep_crossing_multiple_boundaries_sounds_only_once() {
        let mut schedule = ReminderSchedule::new(9_000, 5_000);
        assert!(schedule.crossed_boundary(36_000, 5_000));
        assert_eq!(schedule.next_boundary_ms, Some(40_000));
        assert!(!schedule.crossed_boundary(36_000, 5_000));
    }

    #[test]
    fn launch_schedules_after_elapsed_time_without_catch_up_sound() {
        let mut schedule = ReminderSchedule::new(36_000, 5_000);
        assert_eq!(schedule.next_boundary_ms, Some(40_000));
        assert!(!schedule.crossed_boundary(36_000, 5_000));
        assert!(schedule.crossed_boundary(40_000, 5_000));
    }

    #[test]
    fn alarm_sounds_once_and_launch_skips_a_passed_alarm() {
        let mut schedule = AlarmSchedule::new(9_000, 10_000);
        assert!(!schedule.crossed_boundary(9_999, 10_000));
        assert!(schedule.crossed_boundary(10_000, 10_000));
        assert!(!schedule.crossed_boundary(20_000, 10_000));

        let mut relaunched = AlarmSchedule::new(12_000, 10_000);
        assert!(!relaunched.crossed_boundary(12_000, 10_000));

        let mut reset = AlarmSchedule::new(0, 10_000);
        assert!(reset.crossed_boundary(10_000, 10_000));
    }

    #[test]
    fn version_one_timers_migrate_as_expanded() {
        let path = std::env::temp_dir().join(format!("sticky-timer-v1-{}.json", Uuid::new_v4()));
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "timers": {
                    "timer": {
                        "id": "timer",
                        "accumulated_ms": 0,
                        "running_since_ms": null,
                        "reminder_interval_ms": 0,
                        "alarm_at_ms": 0,
                        "pinned": false,
                        "x": 10,
                        "y": 20
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let migrated = read_and_validate_store(&path).unwrap();
        assert_eq!(migrated.version, TIMER_STORE_VERSION);
        assert!(!migrated.timers["timer"].collapsed);
        fs::remove_file(path).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn alarm_sequence_is_seven_triplets_with_required_silence() {
        let sound_duration_ms = 571;
        let starts = alarm_playback_starts(sound_duration_ms);
        assert_eq!(starts.len(), 21);
        for group in 0..ALARM_GROUP_COUNT {
            let triplet = &starts[group * 3..group * 3 + 3];
            assert_eq!(triplet[1] - triplet[0], ALARM_INTRA_GROUP_MS);
            assert_eq!(triplet[2] - triplet[1], ALARM_INTRA_GROUP_MS);
            if group + 1 < ALARM_GROUP_COUNT {
                assert_eq!(
                    starts[(group + 1) * 3] - triplet[2],
                    sound_duration_ms + ALARM_INTER_GROUP_SILENCE_MS
                );
            }
        }
    }
}
