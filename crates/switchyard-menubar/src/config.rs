// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Menu bar settings, read from `~/.switchyard/menubar.toml`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::pricing::PriceTable;

/// Directory the installer writes every daemon file into.
pub const HOME_DIR_NAME: &str = ".switchyard";

fn default_server_url() -> String {
    "http://127.0.0.1:4123".to_string()
}

fn default_routing_log() -> PathBuf {
    PathBuf::from("~/.switchyard/routing.jsonl")
}

fn default_config_file() -> PathBuf {
    PathBuf::from("~/.switchyard/composite.toml")
}

fn default_launchd_label() -> String {
    "com.nvidia.switchyard.server".to_string()
}

fn default_refresh_seconds() -> u64 {
    30
}

fn default_baseline_model() -> String {
    "gpt-5.6-sol".to_string()
}

/// Everything the menu bar needs to find the server and price its traffic.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Base URL the server listens on, used for the health check.
    #[serde(default = "default_server_url")]
    pub server_url: String,
    /// JSONL file the server appends routing records to.
    #[serde(default = "default_routing_log")]
    pub routing_log: PathBuf,
    /// Server TOML, opened by the "Open config" menu item.
    #[serde(default = "default_config_file")]
    pub config_file: PathBuf,
    /// LaunchAgent label used to restart the server.
    #[serde(default = "default_launchd_label")]
    pub launchd_label: String,
    /// How often the menu contents are recomputed.
    #[serde(default = "default_refresh_seconds")]
    pub refresh_seconds: u64,
    /// Model the traffic is assumed to have used without Switchyard.
    #[serde(default = "default_baseline_model")]
    pub baseline_model: String,
    /// Per-model rates. Dollar figures are hidden while a seen model is absent.
    #[serde(default)]
    pub prices: PriceTable,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: default_server_url(),
            routing_log: default_routing_log(),
            config_file: default_config_file(),
            launchd_label: default_launchd_label(),
            refresh_seconds: default_refresh_seconds(),
            baseline_model: default_baseline_model(),
            prices: PriceTable::new(),
        }
    }
}

impl Config {
    /// Parses settings, falling back to defaults when the file is absent.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("read {}: {error}", path.display())),
        };
        let mut config: Self =
            toml::from_str(&text).map_err(|error| format!("parse {}: {error}", path.display()))?;
        config.routing_log = expand_home(&config.routing_log);
        config.config_file = expand_home(&config.config_file);
        Ok(config)
    }

    /// Default settings path, `~/.switchyard/menubar.toml`.
    pub fn default_path() -> PathBuf {
        home_dir().join(HOME_DIR_NAME).join("menubar.toml")
    }
}

/// The user's home directory, or the current directory when `HOME` is unset.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Rewrites a leading `~` to the home directory. Paths are written by hand in
/// the settings file, where `~` is the natural way to spell a home path.
pub fn expand_home(path: &Path) -> PathBuf {
    expand_under(path, &home_dir())
}

fn expand_under(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_settings_and_prices() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("menubar.toml");
        std::fs::write(
            &path,
            r#"
server_url = "http://127.0.0.1:9000"
baseline_model = "sol"

[prices.sol]
input_per_mtok = 1.25
output_per_mtok = 10.0

[prices.luna]
input_per_mtok = 0.25
cached_input_per_mtok = 0.025
output_per_mtok = 2.0
"#,
        )
        .expect("write settings");

        let config = Config::load(&path).expect("load");

        assert_eq!(config.server_url, "http://127.0.0.1:9000");
        assert_eq!(config.baseline_model, "sol");
        assert_eq!(config.prices["luna"].cached_input_per_mtok, Some(0.025));
        assert_eq!(config.prices["sol"].cached_input_per_mtok, None);
        assert_eq!(config.refresh_seconds, 30, "unset keys keep their default");
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");

        let config = Config::load(&dir.path().join("absent.toml")).expect("load");

        assert_eq!(config.server_url, "http://127.0.0.1:4123");
        assert!(config.prices.is_empty());
    }

    #[test]
    fn rejects_unparsable_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("menubar.toml");
        std::fs::write(&path, "server_url = ").expect("write settings");

        assert!(Config::load(&path).is_err());
    }

    #[test]
    fn expands_a_leading_tilde() {
        let home = Path::new("/Users/example");

        assert_eq!(
            expand_under(Path::new("~/.switchyard/routing.jsonl"), home),
            PathBuf::from("/Users/example/.switchyard/routing.jsonl")
        );
        assert_eq!(
            expand_under(Path::new("/tmp/routing.jsonl"), home),
            PathBuf::from("/tmp/routing.jsonl")
        );
    }
}
