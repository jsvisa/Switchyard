// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Turns rollups and prices into the rows the menu shows.

use crate::config::Config;
use crate::health::ServerStatus;
use crate::pricing::{Savings, estimate};
use crate::rollup::DayTotals;

/// Most model rows shown under a period, so the menu stays short.
const MAX_MODEL_ROWS: usize = 5;

/// One line of the menu. Rows are informational; the actions are fixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Separator,
    Label(String),
}

impl Row {
    fn label(text: impl Into<String>) -> Self {
        Self::Label(text.into())
    }
}

/// Everything the tray needs to redraw itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    /// Menu rows, top to bottom, above the action items.
    pub rows: Vec<Row>,
    /// Hover text on the menu bar icon.
    pub tooltip: String,
}

/// Builds the menu body for the current day and trailing week.
pub fn build(
    status: ServerStatus,
    today: &DayTotals,
    week: &DayTotals,
    config: &Config,
) -> Summary {
    let mut rows = vec![
        Row::label(status_text(status, &config.server_url)),
        Row::Separator,
    ];

    if today.is_empty() && week.is_empty() {
        rows.push(Row::label("No requests recorded yet"));
        return Summary {
            rows,
            tooltip: format!("Switchyard — {}", status.as_str()),
        };
    }

    let today_savings = estimate(today, &config.prices, &config.baseline_model);
    let week_savings = estimate(week, &config.prices, &config.baseline_model);

    rows.extend(period_rows("Today", today, today_savings));
    rows.push(Row::Separator);
    rows.extend(period_rows("This week", week, week_savings));

    let model_rows = model_share_rows(week);
    if !model_rows.is_empty() {
        rows.push(Row::Separator);
        rows.push(Row::label("This week by model"));
        rows.extend(model_rows);
    }

    if config.prices.is_empty() {
        rows.push(Row::Separator);
        rows.push(Row::label("Add prices to menubar.toml to see savings"));
    }

    Summary {
        rows,
        tooltip: tooltip(today, today_savings),
    }
}

fn status_text(status: ServerStatus, server_url: &str) -> String {
    match status {
        ServerStatus::Running => format!("Server: running · {server_url}"),
        ServerStatus::Stopped => format!("Server: not responding · {server_url}"),
    }
}

fn period_rows(label: &str, totals: &DayTotals, savings: Option<Savings>) -> Vec<Row> {
    let requests = totals.requests();
    let mut rows = vec![Row::label(format!(
        "{label} — {} request{} · {} tokens",
        format_count(requests),
        if requests == 1 { "" } else { "s" },
        format_tokens(totals.tokens())
    ))];
    if let Some(savings) = savings {
        rows.push(Row::label(format!(
            "    Saved {}{}",
            format_money(savings.saved()),
            savings
                .percent()
                .map(|percent| format!(" ({percent:.0}% of {})", format_money(savings.baseline)))
                .unwrap_or_default()
        )));
    }
    rows
}

fn model_share_rows(week: &DayTotals) -> Vec<Row> {
    let total: u64 = week.routed.values().map(|tokens| tokens.total()).sum();
    if total == 0 {
        return Vec::new();
    }
    let mut models: Vec<_> = week
        .routed
        .iter()
        .map(|(model, tokens)| (model.clone(), tokens.total()))
        .collect();
    // Largest share first; model id breaks ties so the order is stable.
    models.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    models.truncate(MAX_MODEL_ROWS);
    models
        .into_iter()
        .map(|(model, tokens)| {
            Row::label(format!(
                "    {model} — {:.0}%",
                tokens as f64 / total as f64 * 100.0
            ))
        })
        .collect()
}

fn tooltip(today: &DayTotals, savings: Option<Savings>) -> String {
    let base = format!(
        "Switchyard — today: {} request{}, {} tokens",
        format_count(today.requests()),
        if today.requests() == 1 { "" } else { "s" },
        format_tokens(today.tokens())
    );
    match savings {
        Some(savings) => format!("{base}, {} saved", format_money(savings.saved())),
        None => base,
    }
}

/// Formats a count with thousands separators.
fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Formats a token count compactly, since exact totals are not useful here.
fn format_tokens(value: u64) -> String {
    match value {
        0..=9_999 => format_count(value),
        10_000..=999_999 => format!("{:.0}K", value as f64 / 1_000.0),
        1_000_000..=999_999_999 => format!("{:.1}M", value as f64 / 1_000_000.0),
        _ => format!("{:.1}B", value as f64 / 1_000_000_000.0),
    }
}

/// Formats dollars, keeping the sign readable when routing cost more than it saved.
fn format_money(value: f64) -> String {
    if value < 0.0 {
        format!("-${:.2}", -value)
    } else {
        format!("${value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{ModelPrice, PriceTable};
    use crate::rollup::ModelTokens;

    fn tokens(fresh: u64, output: u64, requests: u64) -> ModelTokens {
        ModelTokens {
            requests,
            fresh_input: fresh,
            cached_input: 0,
            cache_write: 0,
            output,
        }
    }

    fn config(prices: PriceTable) -> Config {
        Config {
            baseline_model: "sol".to_string(),
            prices,
            ..Config::default()
        }
    }

    fn priced() -> PriceTable {
        PriceTable::from([
            (
                "sol".to_string(),
                ModelPrice {
                    input_per_mtok: 1.25,
                    cached_input_per_mtok: None,
                    output_per_mtok: 10.0,
                },
            ),
            (
                "luna".to_string(),
                ModelPrice {
                    input_per_mtok: 0.25,
                    cached_input_per_mtok: None,
                    output_per_mtok: 2.0,
                },
            ),
        ])
    }

    fn labels(summary: &Summary) -> Vec<String> {
        summary
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Label(text) => Some(text.clone()),
                Row::Separator => None,
            })
            .collect()
    }

    #[test]
    fn shows_requests_tokens_and_savings_for_both_periods() {
        let mut today = DayTotals::default();
        today
            .routed
            .insert("luna".to_string(), tokens(1_000_000, 100_000, 3));
        let mut week = DayTotals::default();
        week.routed
            .insert("luna".to_string(), tokens(4_000_000, 400_000, 12));

        let summary = build(ServerStatus::Running, &today, &week, &config(priced()));
        let labels = labels(&summary);

        assert_eq!(labels[0], "Server: running · http://127.0.0.1:4123");
        assert_eq!(labels[1], "Today — 3 requests · 1.1M tokens");
        assert_eq!(labels[2], "    Saved $1.80 (80% of $2.25)");
        assert_eq!(labels[3], "This week — 12 requests · 4.4M tokens");
        assert_eq!(labels[4], "    Saved $7.20 (80% of $9.00)");
    }

    #[test]
    fn hides_savings_and_explains_why_when_prices_are_missing() {
        let mut today = DayTotals::default();
        today
            .routed
            .insert("luna".to_string(), tokens(1_000, 100, 1));

        let summary = build(
            ServerStatus::Running,
            &today,
            &today,
            &config(PriceTable::new()),
        );
        let labels = labels(&summary);

        assert!(labels.iter().all(|label| !label.contains("Saved")));
        assert!(labels.iter().any(|label| label.contains("menubar.toml")));
    }

    #[test]
    fn ranks_models_by_token_share() {
        let mut week = DayTotals::default();
        week.routed
            .insert("luna".to_string(), tokens(700_000, 0, 7));
        week.routed.insert("sol".to_string(), tokens(300_000, 0, 3));

        let summary = build(
            ServerStatus::Running,
            &DayTotals::default(),
            &week,
            &config(priced()),
        );
        let labels = labels(&summary);
        let models: Vec<_> = labels
            .iter()
            .filter(|label| label.contains('%') && label.contains('—') && label.starts_with("    "))
            .cloned()
            .collect();

        assert_eq!(models, vec!["    luna — 70%", "    sol — 30%"]);
    }

    #[test]
    fn reports_a_stopped_server() {
        let summary = build(
            ServerStatus::Stopped,
            &DayTotals::default(),
            &DayTotals::default(),
            &config(priced()),
        );
        let labels = labels(&summary);

        assert!(labels[0].starts_with("Server: not responding"));
        assert_eq!(labels[1], "No requests recorded yet");
    }

    #[test]
    fn shows_a_loss_when_routing_overhead_exceeds_the_saving() {
        let mut today = DayTotals::default();
        today.routed.insert("sol".to_string(), tokens(1_000, 0, 1));
        today
            .classifier
            .insert("sol".to_string(), tokens(1_000_000, 0, 1));

        let summary = build(ServerStatus::Running, &today, &today, &config(priced()));

        assert!(labels(&summary)[2].starts_with("    Saved -$1.25"));
    }

    #[test]
    fn formats_counts_and_tokens_for_reading() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(412), "412");
        assert_eq!(format_count(2_904), "2,904");
        assert_eq!(format_count(1_234_567), "1,234,567");

        assert_eq!(format_tokens(412), "412");
        assert_eq!(format_tokens(12_345), "12K");
        assert_eq!(format_tokens(1_400_000), "1.4M");
        assert_eq!(format_tokens(9_700_000_000), "9.7B");
    }

    #[test]
    fn formats_money_with_a_readable_sign() {
        assert_eq!(format_money(2.181), "$2.18");
        assert_eq!(format_money(0.0), "$0.00");
        assert_eq!(format_money(-0.125), "-$0.12");
    }
}
