// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Menu bar companion for a locally running Switchyard server.
//!
//! The server owns routing; this process only reads what the server already
//! wrote. It shows today's and this week's traffic, and what that traffic
//! would have cost had every call gone to the capable model instead.

mod app;
mod config;
mod health;
mod icon;
mod pricing;
mod rollup;
mod summary;
#[cfg(target_os = "macos")]
mod tray;

use std::path::PathBuf;
use std::process::ExitCode;

use config::Config;

const USAGE: &str = "\
Usage: switchyard-menubar [OPTIONS] [SETTINGS_FILE]

Shows Switchyard usage and estimated savings in the macOS menu bar.

Arguments:
  SETTINGS_FILE  Settings TOML [default: ~/.switchyard/menubar.toml]

Options:
      --print  Print the current summary and exit, instead of running
  -h, --help   Show this help";

/// Parsed command line.
struct Args {
    settings: Option<PathBuf>,
    print_only: bool,
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("switchyard-menubar: {error}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let path = args.settings.unwrap_or_else(Config::default_path);
    let config = match Config::load(&path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("switchyard-menubar: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut app = app::App::new(config);
    if args.print_only {
        print_summary(&mut app);
        return ExitCode::SUCCESS;
    }
    run(app)
}

/// Returns `Ok(None)` when help was asked for.
fn parse(args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut parsed = Args {
        settings: None,
        print_only: false,
    };
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--print" => parsed.print_only = true,
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other if parsed.settings.is_some() => {
                return Err(format!("unexpected argument {other}"));
            }
            other => parsed.settings = Some(PathBuf::from(other)),
        }
    }
    Ok(Some(parsed))
}

/// Writes the menu's rows to stdout. Useful for checking a config without a
/// menu bar, and the only output this binary has off macOS.
fn print_summary(app: &mut app::App) {
    for row in app.refresh().rows {
        match row {
            summary::Row::Separator => println!(),
            summary::Row::Label(text) => println!("{text}"),
        }
    }
}

#[cfg(target_os = "macos")]
fn run(app: app::App) -> ExitCode {
    match tray::run(app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("switchyard-menubar: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Off macOS there is no menu bar to attach to. The rollup and pricing logic
/// still builds and runs, so `--print` is the whole program there.
#[cfg(not(target_os = "macos"))]
fn run(mut app: app::App) -> ExitCode {
    print_summary(&mut app);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Option<Args>, String> {
        parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn defaults_to_running_with_the_default_settings_path() {
        let args = parse_args(&[]).expect("parse").expect("not help");

        assert!(args.settings.is_none());
        assert!(!args.print_only);
    }

    #[test]
    fn accepts_a_settings_path_and_print() {
        let args = parse_args(&["--print", "/tmp/menubar.toml"])
            .expect("parse")
            .expect("not help");

        assert!(args.print_only);
        assert_eq!(args.settings, Some(PathBuf::from("/tmp/menubar.toml")));
    }

    #[test]
    fn help_short_circuits() {
        assert!(parse_args(&["--help"]).expect("parse").is_none());
        assert!(parse_args(&["-h"]).expect("parse").is_none());
    }

    #[test]
    fn rejects_unknown_options_and_extra_arguments() {
        assert!(parse_args(&["--nope"]).is_err());
        assert!(parse_args(&["a.toml", "b.toml"]).is_err());
    }
}
