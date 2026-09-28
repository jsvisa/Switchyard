// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Per-day token rollups over the server's routing JSONL log.
//!
//! The log is append-only and grows for the life of the install, so the reader
//! keeps its byte offset and folds only newly written lines on each refresh.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Local, NaiveDate};
use serde::Deserialize;

/// Days of history kept in memory. One more than a week so a full trailing
/// week stays available right after local midnight.
const RETAINED_DAYS: i64 = 8;

/// Tier value the server writes for classifier and judge calls.
const CLASSIFIER_TIER: &str = "classifier";

/// Billable token counts for one model.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModelTokens {
    pub requests: u64,
    /// Input tokens billed at the full input rate.
    pub fresh_input: u64,
    /// Input tokens served from the provider cache.
    pub cached_input: u64,
    /// Input tokens written into the provider cache.
    pub cache_write: u64,
    /// Generated tokens, including reasoning.
    pub output: u64,
}

impl ModelTokens {
    /// Every token the provider counted, cached reads included.
    pub fn total(&self) -> u64 {
        self.fresh_input
            .saturating_add(self.cached_input)
            .saturating_add(self.cache_write)
            .saturating_add(self.output)
    }

    fn add(&mut self, other: &Self) {
        self.requests = self.requests.saturating_add(other.requests);
        self.fresh_input = self.fresh_input.saturating_add(other.fresh_input);
        self.cached_input = self.cached_input.saturating_add(other.cached_input);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.output = self.output.saturating_add(other.output);
    }
}

/// Tokens for one day, split by whether Switchyard or the caller asked for them.
///
/// The split matters for savings: without Switchyard the `routed` calls would
/// still have happened, but the `classifier` calls would not exist at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DayTotals {
    /// Calls that served the caller, keyed by the model that answered.
    pub routed: BTreeMap<String, ModelTokens>,
    /// Switchyard's own classifier and judge calls, keyed by model.
    pub classifier: BTreeMap<String, ModelTokens>,
}

impl DayTotals {
    fn add(&mut self, other: &Self) {
        for (model, tokens) in &other.routed {
            self.routed.entry(model.clone()).or_default().add(tokens);
        }
        for (model, tokens) in &other.classifier {
            self.classifier
                .entry(model.clone())
                .or_default()
                .add(tokens);
        }
    }

    /// Calls made on the caller's behalf.
    pub fn requests(&self) -> u64 {
        self.routed.values().map(|tokens| tokens.requests).sum()
    }

    /// Every token spent, Switchyard's own routing overhead included.
    pub fn tokens(&self) -> u64 {
        self.routed
            .values()
            .chain(self.classifier.values())
            .map(ModelTokens::total)
            .sum()
    }

    /// True when nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.routed.is_empty() && self.classifier.is_empty()
    }
}

/// Fields the rollup needs from one routing log line.
#[derive(Deserialize)]
struct RoutingRecord {
    ts: String,
    model: String,
    #[serde(default)]
    tier: String,
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    cached_tokens: u64,
    #[serde(default)]
    cache_creation_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
}

impl RoutingRecord {
    fn tokens(&self) -> ModelTokens {
        // `prompt_tokens` is the whole input, cache detail included, so the
        // fully-priced remainder is what is left after removing that detail.
        let cache_detail = self
            .cached_tokens
            .saturating_add(self.cache_creation_tokens);
        ModelTokens {
            requests: 1,
            fresh_input: self.prompt_tokens.saturating_sub(cache_detail),
            cached_input: self.cached_tokens,
            cache_write: self.cache_creation_tokens,
            output: self.completion_tokens.saturating_add(self.reasoning_tokens),
        }
    }
}

/// Folds routing log lines into local-date buckets, resuming where it stopped.
#[derive(Debug, Default)]
pub struct RollupReader {
    offset: u64,
    days: BTreeMap<NaiveDate, DayTotals>,
}

impl RollupReader {
    /// Creates a reader positioned at the start of the log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds in every complete line written since the last refresh.
    ///
    /// A missing log is not an error: the server only creates it once it has
    /// served a request.
    pub fn refresh(&mut self, path: &Path, today: NaiveDate) -> std::io::Result<()> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };

        // A shorter file means the log was rotated or truncated underneath us,
        // so the buckets no longer describe what is on disk.
        if file.metadata()?.len() < self.offset {
            self.offset = 0;
            self.days.clear();
        }

        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line)?;
            if read == 0 {
                break;
            }
            // A line without its newline is still being written; leave the
            // offset before it so the next refresh sees it whole.
            if !line.ends_with(b"\n") {
                break;
            }
            self.offset = self.offset.saturating_add(read as u64);
            if let Ok(record) = serde_json::from_slice::<RoutingRecord>(&line) {
                self.add(&record);
            }
        }

        self.prune(today);
        Ok(())
    }

    fn add(&mut self, record: &RoutingRecord) {
        let Ok(ts) = DateTime::parse_from_rfc3339(&record.ts) else {
            return;
        };
        let day = ts.with_timezone(&Local).date_naive();
        let totals = self.days.entry(day).or_default();
        let bucket = if record.tier == CLASSIFIER_TIER {
            &mut totals.classifier
        } else {
            &mut totals.routed
        };
        bucket
            .entry(record.model.clone())
            .or_default()
            .add(&record.tokens());
    }

    fn prune(&mut self, today: NaiveDate) {
        let cutoff = today - chrono::Duration::days(RETAINED_DAYS - 1);
        self.days.retain(|day, _| *day >= cutoff);
    }

    /// Totals for a single local date.
    pub fn day(&self, day: NaiveDate) -> DayTotals {
        self.days.get(&day).cloned().unwrap_or_default()
    }

    /// Totals for the seven local days ending on `today`, inclusive.
    pub fn week_ending(&self, today: NaiveDate) -> DayTotals {
        let start = today - chrono::Duration::days(6);
        let mut totals = DayTotals::default();
        for (day, day_totals) in &self.days {
            if *day >= start && *day <= today {
                totals.add(day_totals);
            }
        }
        totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn date(text: &str) -> NaiveDate {
        text.parse().expect("valid date")
    }

    /// Builds a log line stamped at a local time, mirroring how the server
    /// writes UTC timestamps for events that happened in the local day.
    fn line(local_ts: &str, model: &str, tier: &str, prompt: u64, completion: u64) -> String {
        let offset = chrono::Local::now().offset().to_string();
        format!(
            r#"{{"ts":"{local_ts}{offset}","route_id":"switchyard","algorithm":"composite","model":"{model}","tier":"{tier}","prompt_tokens":{prompt},"cached_tokens":0,"cache_creation_tokens":0,"completion_tokens":{completion},"reasoning_tokens":0,"total_tokens":0}}"#
        )
    }

    fn write(path: &Path, contents: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open log");
        file.write_all(contents.as_bytes()).expect("write log");
    }

    #[test]
    fn splits_routed_and_classifier_tokens() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("routing.jsonl");
        write(
            &path,
            &format!(
                "{}\n{}\n",
                line("2026-09-28T10:00:00.000", "gpt-5.6-luna", "", 1_000, 200),
                line(
                    "2026-09-28T10:00:01.000",
                    "gpt-5.6-terra",
                    "classifier",
                    300,
                    10
                ),
            ),
        );

        let mut reader = RollupReader::new();
        reader.refresh(&path, date("2026-09-28")).expect("refresh");
        let day = reader.day(date("2026-09-28"));

        assert_eq!(
            day.requests(),
            1,
            "classifier calls are not caller requests"
        );
        assert_eq!(day.routed["gpt-5.6-luna"].fresh_input, 1_000);
        assert_eq!(day.routed["gpt-5.6-luna"].output, 200);
        assert_eq!(day.classifier["gpt-5.6-terra"].fresh_input, 300);
        assert_eq!(day.tokens(), 1_000 + 200 + 300 + 10);
    }

    #[test]
    fn separates_cache_detail_from_fresh_input() {
        let record: RoutingRecord = serde_json::from_str(
            r#"{"ts":"2026-09-28T10:00:00.000Z","model":"m","tier":"","prompt_tokens":1000,"cached_tokens":600,"cache_creation_tokens":100,"completion_tokens":50,"reasoning_tokens":25}"#,
        )
        .expect("parse record");
        let tokens = record.tokens();

        assert_eq!(tokens.fresh_input, 300);
        assert_eq!(tokens.cached_input, 600);
        assert_eq!(tokens.cache_write, 100);
        assert_eq!(tokens.output, 75, "reasoning tokens are billed as output");
        assert_eq!(tokens.total(), 1_075);
    }

    #[test]
    fn folds_only_new_lines_on_refresh() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("routing.jsonl");
        write(
            &path,
            &format!("{}\n", line("2026-09-28T10:00:00.000", "luna", "", 100, 10)),
        );

        let mut reader = RollupReader::new();
        reader.refresh(&path, date("2026-09-28")).expect("first");
        reader.refresh(&path, date("2026-09-28")).expect("second");
        assert_eq!(reader.day(date("2026-09-28")).requests(), 1);

        write(
            &path,
            &format!("{}\n", line("2026-09-28T11:00:00.000", "luna", "", 100, 10)),
        );
        reader.refresh(&path, date("2026-09-28")).expect("third");
        assert_eq!(reader.day(date("2026-09-28")).requests(), 2);
    }

    #[test]
    fn leaves_a_partial_trailing_line_for_the_next_refresh() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("routing.jsonl");
        let complete = line("2026-09-28T10:00:00.000", "luna", "", 100, 10);
        write(&path, &format!("{complete}\n"));
        // The server is mid-write: the second record has no newline yet.
        let partial = line("2026-09-28T10:00:05.000", "luna", "", 500, 50);
        let (head, tail) = partial.split_at(20);
        write(&path, head);

        let mut reader = RollupReader::new();
        reader.refresh(&path, date("2026-09-28")).expect("partial");
        assert_eq!(reader.day(date("2026-09-28")).requests(), 1);

        write(&path, &format!("{tail}\n"));
        reader.refresh(&path, date("2026-09-28")).expect("complete");
        assert_eq!(reader.day(date("2026-09-28")).requests(), 2);
    }

    #[test]
    fn rereads_from_the_start_when_the_log_is_truncated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("routing.jsonl");
        write(
            &path,
            &format!(
                "{}\n{}\n",
                line("2026-09-28T10:00:00.000", "luna", "", 100, 10),
                line("2026-09-28T10:00:01.000", "luna", "", 100, 10),
            ),
        );
        let mut reader = RollupReader::new();
        reader.refresh(&path, date("2026-09-28")).expect("first");
        assert_eq!(reader.day(date("2026-09-28")).requests(), 2);

        std::fs::write(
            &path,
            format!("{}\n", line("2026-09-28T12:00:00.000", "luna", "", 100, 10)),
        )
        .expect("truncate log");
        reader.refresh(&path, date("2026-09-28")).expect("second");
        assert_eq!(reader.day(date("2026-09-28")).requests(), 1);
    }

    #[test]
    fn week_covers_seven_days_and_drops_older_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("routing.jsonl");
        let mut contents = String::new();
        // One request a day for eleven days ending on the 28th.
        for day in 18..=28 {
            contents.push_str(&line(
                &format!("2026-09-{day:02}T10:00:00.000"),
                "luna",
                "",
                100,
                10,
            ));
            contents.push('\n');
        }
        write(&path, &contents);

        let mut reader = RollupReader::new();
        reader.refresh(&path, date("2026-09-28")).expect("refresh");

        assert_eq!(reader.week_ending(date("2026-09-28")).requests(), 7);
        assert_eq!(reader.day(date("2026-09-28")).requests(), 1);
        assert!(
            reader.day(date("2026-09-20")).is_empty(),
            "days beyond the retention window are pruned"
        );
    }

    #[test]
    fn skips_unparsable_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("routing.jsonl");
        write(
            &path,
            &format!(
                "not json\n{}\n",
                line("2026-09-28T10:00:00.000", "luna", "", 100, 10)
            ),
        );

        let mut reader = RollupReader::new();
        reader.refresh(&path, date("2026-09-28")).expect("refresh");
        assert_eq!(reader.day(date("2026-09-28")).requests(), 1);
    }

    #[test]
    fn a_missing_log_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut reader = RollupReader::new();
        reader
            .refresh(&dir.path().join("absent.jsonl"), date("2026-09-28"))
            .expect("missing log is fine");
        assert!(reader.day(date("2026-09-28")).is_empty());
    }
}
