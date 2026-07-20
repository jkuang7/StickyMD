use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{bail, Context};
use tauri::{AppHandle, Manager};

use crate::save_load::{note_id_from_label, NoteRepository};

pub(crate) const PINNED_WINDOW_REGISTRY: &str = "aerospace-pinned-windows.tsv";
const REGISTRY_HEADER: &str = "sticky-pinned-windows-v1";
static TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

fn serialize_registry(entries: &mut Vec<(i64, String)>) -> anyhow::Result<String> {
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    entries.dedup();

    let mut registry = format!("{REGISTRY_HEADER}\n");
    for (window_id, note_id) in entries {
        if *window_id <= 0 || note_id.is_empty() || note_id.contains(['\t', '\r', '\n']) {
            bail!("Invalid pinned-window registry entry for note {note_id:?}");
        }
        registry.push_str(&format!("{window_id}\t{note_id}\n"));
    }
    Ok(registry)
}

fn write_registry_atomically(parent: &Path, contents: &str) -> anyhow::Result<()> {
    fs::create_dir_all(parent).context("Could not create Sticky application-data directory")?;
    let path = parent.join(PINNED_WINDOW_REGISTRY);
    let temp_id = TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{PINNED_WINDOW_REGISTRY}.tmp-{}-{temp_id}",
        std::process::id()
    ));

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .context("Could not create pinned-window registry temporary file")?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &path).context("Could not replace pinned-window registry")?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(target_os = "macos")]
fn native_window_id(window: &tauri::WebviewWindow) -> anyhow::Result<i64> {
    use objc2_app_kit::NSWindow;

    let ns_window = window.ns_window()?;
    let window_number = unsafe { (&*(ns_window as *const NSWindow)).windowNumber() };
    Ok(window_number as i64)
}

pub(crate) fn sync_pinned_window_registry(
    app: &AppHandle,
    excluded_note_id: Option<&str>,
) -> anyhow::Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, excluded_note_id);
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let repository = app.state::<NoteRepository>();
        let mut entries = Vec::new();
        for window in app.webview_windows().into_values() {
            let Ok(note_id) = note_id_from_label(window.label()) else {
                continue;
            };
            if excluded_note_id == Some(note_id) || !repository.get(note_id)?.pinned {
                continue;
            }
            entries.push((native_window_id(&window)?, note_id.to_string()));
        }

        let contents = serialize_registry(&mut entries)?;
        let app_data_dir = app
            .path()
            .app_data_dir()
            .context("Could not locate Sticky application-data directory")?;
        write_registry_atomically(&app_data_dir, &contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_versioned_sorted_and_deduplicated() {
        let mut entries = vec![
            (42, "second".to_string()),
            (7, "first".to_string()),
            (42, "second".to_string()),
        ];

        assert_eq!(
            serialize_registry(&mut entries).unwrap(),
            "sticky-pinned-windows-v1\n7\tfirst\n42\tsecond\n"
        );
    }

    #[test]
    fn registry_rejects_fields_that_could_change_its_shape() {
        let mut entries = vec![(7, "bad\tnote".to_string())];
        assert!(serialize_registry(&mut entries).is_err());
    }
}
