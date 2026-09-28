// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! macOS status item: owns the event loop, the menu, and the refresh worker.
//!
//! AppKit work has to stay on the main thread, and probing the server can
//! block for seconds, so a worker thread owns the [`App`] and sends finished
//! summaries back for the main thread to draw.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
use objc2_foundation::{MainThreadMarker, NSDate, NSDefaultRunLoopMode};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::app::App;
use crate::icon;
use crate::summary::{Row, Summary};

const RESTART_ID: &str = "restart-server";
const OPEN_CONFIG_ID: &str = "open-config";
const OPEN_SETTINGS_ID: &str = "open-settings";
const QUIT_ID: &str = "quit";

/// Shortest refresh the settings file may ask for.
const MIN_REFRESH: Duration = Duration::from_secs(5);

/// How long the main loop blocks waiting for a UI event before looking for a
/// new summary. Short enough that the menu redraws promptly after an action.
const EVENT_POLL: f64 = 0.1;

/// What the main thread asks the worker to do.
enum Command {
    RestartServer,
    OpenConfig,
    OpenSettings,
}

/// Runs the status item until the user quits.
pub fn run(app: App) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("the menu bar must run on the main thread")?;
    let ns_app = NSApplication::sharedApplication(mtm);
    // Accessory keeps the process out of the Dock and the app switcher.
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let (command_tx, summary_rx) = spawn_worker(app);
    let tray = build_tray()?;

    ns_app.finishLaunching();

    loop {
        pump_events(&ns_app);

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let command = match event.id.as_ref() {
                QUIT_ID => return Ok(()),
                RESTART_ID => Command::RestartServer,
                OPEN_CONFIG_ID => Command::OpenConfig,
                OPEN_SETTINGS_ID => Command::OpenSettings,
                // Informational rows are disabled, so nothing else fires.
                _ => continue,
            };
            if command_tx.send(command).is_err() {
                return Err("the refresh worker stopped".to_string());
            }
        }

        while let Ok(summary) = summary_rx.try_recv() {
            apply(&tray, &summary);
        }
    }
}

/// Starts the thread that owns the rollup and does every blocking call.
fn spawn_worker(mut app: App) -> (Sender<Command>, Receiver<Summary>) {
    let (command_tx, command_rx) = channel::<Command>();
    let (summary_tx, summary_rx) = channel::<Summary>();
    let interval = Duration::from_secs(app.config().refresh_seconds).max(MIN_REFRESH);

    std::thread::spawn(move || {
        loop {
            if summary_tx.send(app.refresh()).is_err() {
                return;
            }
            match command_rx.recv_timeout(interval) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(Command::RestartServer) => report(app.restart_server()),
                Ok(Command::OpenConfig) => report(app.open_config()),
                Ok(Command::OpenSettings) => report(app.open_settings()),
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });

    (command_tx, summary_rx)
}

fn report(result: Result<(), String>) {
    if let Err(error) = result {
        eprintln!("switchyard-menubar: {error}");
    }
}

fn build_tray() -> Result<TrayIcon, String> {
    let icon = Icon::from_rgba(icon::glyph(), icon::SIZE, icon::SIZE)
        .map_err(|error| format!("build icon: {error}"))?;
    TrayIconBuilder::new()
        .with_icon(icon)
        .with_icon_as_template(true)
        .with_tooltip("Switchyard")
        .with_menu(Box::new(menu(None)?))
        .build()
        .map_err(|error| format!("create status item: {error}"))
}

fn apply(tray: &TrayIcon, summary: &Summary) {
    match menu(Some(summary)) {
        Ok(menu) => tray.set_menu(Some(Box::new(menu))),
        Err(error) => eprintln!("switchyard-menubar: {error}"),
    }
    if let Err(error) = tray.set_tooltip(Some(&summary.tooltip)) {
        eprintln!("switchyard-menubar: set tooltip: {error}");
    }
}

/// Builds the whole menu: the summary rows, then the fixed actions.
fn menu(summary: Option<&Summary>) -> Result<Menu, String> {
    let menu = Menu::new();
    let append = |item: &dyn tray_icon::menu::IsMenuItem| {
        menu.append(item)
            .map_err(|error| format!("build menu: {error}"))
    };

    match summary {
        // Rows are labels, not commands, so they are disabled.
        Some(summary) => {
            for row in &summary.rows {
                match row {
                    Row::Separator => append(&PredefinedMenuItem::separator())?,
                    Row::Label(text) => append(&MenuItem::new(text, false, None))?,
                }
            }
        }
        None => append(&MenuItem::new("Loading…", false, None))?,
    }

    append(&PredefinedMenuItem::separator())?;
    append(&MenuItem::with_id(RESTART_ID, "Restart server", true, None))?;
    append(&MenuItem::with_id(
        OPEN_CONFIG_ID,
        "Open server config…",
        true,
        None,
    ))?;
    append(&MenuItem::with_id(
        OPEN_SETTINGS_ID,
        "Open menu bar settings…",
        true,
        None,
    ))?;
    append(&PredefinedMenuItem::separator())?;
    append(&MenuItem::with_id(QUIT_ID, "Quit", true, None))?;
    Ok(menu)
}

/// Drains pending AppKit events, blocking briefly when there are none.
fn pump_events(ns_app: &NSApplication) {
    let deadline = NSDate::dateWithTimeIntervalSinceNow(EVENT_POLL);
    let mut expiration = Some(deadline);
    while let Some(event) = unsafe {
        ns_app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            expiration.as_deref(),
            NSDefaultRunLoopMode,
            true,
        )
    } {
        ns_app.sendEvent(&event);
        // Only the first wait blocks; the rest drain what is already queued.
        expiration = Some(NSDate::distantPast());
    }
}
