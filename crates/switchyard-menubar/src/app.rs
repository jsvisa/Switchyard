// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Menu state: refreshes the rollup, probes the server, and runs menu actions.

use std::process::Command;

use chrono::Local;

use crate::config::Config;
use crate::health::probe;
use crate::rollup::RollupReader;
use crate::summary::{Summary, build};

/// Owns the incremental rollup between refreshes.
pub struct App {
    config: Config,
    reader: RollupReader,
}

impl App {
    /// Creates the state for a settings file already loaded from disk.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            reader: RollupReader::new(),
        }
    }

    /// Settings the menu was built from.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Folds in new log lines, probes the server, and rebuilds the menu body.
    pub fn refresh(&mut self) -> Summary {
        let today = Local::now().date_naive();
        if let Err(error) = self.reader.refresh(&self.config.routing_log, today) {
            eprintln!(
                "switchyard-menubar: read {}: {error}",
                self.config.routing_log.display()
            );
        }
        build(
            probe(&self.config.server_url),
            &self.reader.day(today),
            &self.reader.week_ending(today),
            &self.config,
        )
    }

    /// Restarts the server's LaunchAgent.
    pub fn restart_server(&self) -> Result<(), String> {
        let uid = run("id", &["-u"])?;
        let target = format!("gui/{}/{}", uid.trim(), self.config.launchd_label);
        run("launchctl", &["kickstart", "-k", &target]).map(|_| ())
    }

    /// Opens the server TOML in the user's editor.
    pub fn open_config(&self) -> Result<(), String> {
        let path = self.config.config_file.display().to_string();
        run("open", &["-t", &path]).map(|_| ())
    }

    /// Opens the settings TOML in the user's editor.
    pub fn open_settings(&self) -> Result<(), String> {
        let path = Config::default_path().display().to_string();
        run("open", &["-t", &path]).map(|_| ())
    }
}

/// Runs a command, returning its stdout or a message naming what failed.
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("run {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::Row;

    #[test]
    fn refresh_reads_the_configured_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("routing.jsonl");
        let ts = Local::now().to_rfc3339();
        std::fs::write(
            &path,
            format!(
                r#"{{"ts":"{ts}","route_id":"switchyard","algorithm":"composite","model":"luna","tier":"","prompt_tokens":1000,"cached_tokens":0,"cache_creation_tokens":0,"completion_tokens":100,"reasoning_tokens":0,"total_tokens":1100}}"#
            ) + "\n",
        )
        .expect("write log");

        let mut app = App::new(Config {
            routing_log: path,
            ..Config::default()
        });
        let summary = app.refresh();

        assert!(
            summary
                .rows
                .contains(&Row::Label("Today — 1 request · 1,100 tokens".to_string())),
            "unexpected rows: {:?}",
            summary.rows
        );
    }

    #[test]
    fn reports_which_command_failed() {
        let error = run("switchyard-does-not-exist", &[]).expect_err("missing program");

        assert!(error.contains("switchyard-does-not-exist"));
    }
}
