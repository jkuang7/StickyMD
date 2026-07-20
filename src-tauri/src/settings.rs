use std::sync::Mutex;

use anyhow::Context;
use tauri::{menu::CheckMenuItem, AppHandle, Wry};

use crate::menu::MenuCommand;

pub const DEFAULT_FONT_SIZE: u8 = 16;
pub const FONT_SIZE_STEP: u8 = 2;
pub const MIN_FONT_SIZE: u8 = 12;
pub const MAX_FONT_SIZE: u8 = 32;

pub fn clamp_font_size(font_size: i64) -> u8 {
    font_size.clamp(i64::from(MIN_FONT_SIZE), i64::from(MAX_FONT_SIZE)) as u8
}

pub struct MenuSettings {
    pub bring_to_front: CheckMenuItem<Wry>,
    pub autostart: CheckMenuItem<Wry>,
    default_font_size: Mutex<u8>,
}

impl MenuSettings {
    pub fn new(
        app: &AppHandle,
        bring_to_front: bool,
        autostart: bool,
        default_font_size: u8,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            bring_to_front: CheckMenuItem::with_id(
                app,
                MenuCommand::BringToFront,
                "Bring all notes to front on focus",
                true,
                bring_to_front,
                None::<String>,
            )?,
            autostart: CheckMenuItem::with_id(
                app,
                MenuCommand::AutoStart,
                "Launch app on startup",
                true,
                autostart,
                None::<String>,
            )?,
            default_font_size: Mutex::new(clamp_font_size(i64::from(default_font_size))),
        })
    }

    fn get_checked_status(item: &CheckMenuItem<Wry>) -> anyhow::Result<bool> {
        item.is_checked().context("Could not get checked menu item")
    }

    pub fn bring_to_front(&self) -> anyhow::Result<bool> {
        Self::get_checked_status(&self.bring_to_front)
    }

    pub fn autostart(&self) -> anyhow::Result<bool> {
        Self::get_checked_status(&self.autostart)
    }

    pub fn default_font_size(&self) -> anyhow::Result<u8> {
        self.default_font_size
            .lock()
            .map(|font_size| *font_size)
            .map_err(|_| anyhow::anyhow!("Font-size setting lock poisoned"))
    }

    pub fn set_default_font_size(&self, font_size: u8) -> anyhow::Result<u8> {
        let mut current = self
            .default_font_size
            .lock()
            .map_err(|_| anyhow::anyhow!("Font-size setting lock poisoned"))?;
        let previous = *current;
        *current = clamp_font_size(i64::from(font_size));
        Ok(previous)
    }
}
