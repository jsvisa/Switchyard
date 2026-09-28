// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Counterfactual cost accounting: what routed traffic cost against what it
//! would have cost had every call gone to one capable model.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::rollup::{DayTotals, ModelTokens};

const TOKENS_PER_MILLION: f64 = 1_000_000.0;

/// Per-million-token rates for one model.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct ModelPrice {
    /// Rate for input tokens billed in full.
    pub input_per_mtok: f64,
    /// Rate for cache reads. Falls back to the full input rate.
    #[serde(default)]
    pub cached_input_per_mtok: Option<f64>,
    /// Rate for generated tokens, reasoning included.
    pub output_per_mtok: f64,
}

impl ModelPrice {
    fn cost(&self, tokens: &ModelTokens) -> f64 {
        let cached_rate = self.cached_input_per_mtok.unwrap_or(self.input_per_mtok);
        // Cache writes are billed at the full input rate unless a provider
        // charges a separate write rate, which none of the supported ones do.
        let full_input = tokens.fresh_input.saturating_add(tokens.cache_write) as f64;
        (full_input * self.input_per_mtok
            + tokens.cached_input as f64 * cached_rate
            + tokens.output as f64 * self.output_per_mtok)
            / TOKENS_PER_MILLION
    }
}

/// Rates keyed by the model id the server records in the routing log.
pub type PriceTable = BTreeMap<String, ModelPrice>;

/// What the traffic cost, and what it would have cost without routing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Savings {
    /// Cost of the calls that actually ran, classifier overhead included.
    pub actual: f64,
    /// Cost of the same caller-facing calls had they all used the baseline model.
    pub baseline: f64,
}

impl Savings {
    /// Dollars kept. Negative when routing overhead outweighed the cheaper tier.
    pub fn saved(&self) -> f64 {
        self.baseline - self.actual
    }

    /// Fraction of the baseline bill avoided, or `None` with nothing to compare.
    pub fn percent(&self) -> Option<f64> {
        (self.baseline > 0.0).then(|| self.saved() / self.baseline * 100.0)
    }
}

/// Prices a day against the baseline model.
///
/// Returns `None` when any model in the totals has no entry in the table, since
/// a partial bill would understate cost and overstate savings.
pub fn estimate(totals: &DayTotals, prices: &PriceTable, baseline_model: &str) -> Option<Savings> {
    let baseline_price = prices.get(baseline_model)?;

    let mut actual = 0.0;
    let mut baseline = 0.0;
    for (model, tokens) in &totals.routed {
        actual += prices.get(model)?.cost(tokens);
        baseline += baseline_price.cost(tokens);
    }
    // Classifier calls are Switchyard's own cost. They have no baseline
    // counterpart, so they only ever reduce the savings figure.
    for (model, tokens) in &totals.classifier {
        actual += prices.get(model)?.cost(tokens);
    }

    Some(Savings { actual, baseline })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price(input: f64, output: f64) -> ModelPrice {
        ModelPrice {
            input_per_mtok: input,
            cached_input_per_mtok: None,
            output_per_mtok: output,
        }
    }

    fn table() -> PriceTable {
        PriceTable::from([
            ("sol".to_string(), price(1.25, 10.0)),
            ("luna".to_string(), price(0.25, 2.0)),
            ("terra".to_string(), price(0.05, 0.4)),
        ])
    }

    fn tokens(fresh: u64, output: u64) -> ModelTokens {
        ModelTokens {
            requests: 1,
            fresh_input: fresh,
            cached_input: 0,
            cache_write: 0,
            output,
        }
    }

    #[test]
    fn prices_the_cheaper_tier_against_the_capable_baseline() {
        let mut totals = DayTotals::default();
        totals
            .routed
            .insert("luna".to_string(), tokens(1_000_000, 100_000));

        let savings = estimate(&totals, &table(), "sol").expect("priced");

        assert!((savings.actual - (0.25 + 0.2)).abs() < 1e-9);
        assert!((savings.baseline - (1.25 + 1.0)).abs() < 1e-9);
        assert!((savings.saved() - 1.8).abs() < 1e-9);
        assert!((savings.percent().expect("percent") - 80.0).abs() < 1e-9);
    }

    #[test]
    fn classifier_overhead_counts_against_savings() {
        let mut totals = DayTotals::default();
        totals
            .routed
            .insert("luna".to_string(), tokens(1_000_000, 100_000));
        let without_classifier = estimate(&totals, &table(), "sol").expect("priced");

        totals
            .classifier
            .insert("terra".to_string(), tokens(1_000_000, 10_000));
        let with_classifier = estimate(&totals, &table(), "sol").expect("priced");

        assert_eq!(with_classifier.baseline, without_classifier.baseline);
        assert!(with_classifier.actual > without_classifier.actual);
        assert!(with_classifier.saved() < without_classifier.saved());
    }

    #[test]
    fn routing_everything_to_the_baseline_model_saves_nothing() {
        let mut totals = DayTotals::default();
        totals
            .routed
            .insert("sol".to_string(), tokens(1_000_000, 100_000));

        let savings = estimate(&totals, &table(), "sol").expect("priced");

        assert!(savings.saved().abs() < 1e-9);
        assert!(savings.percent().expect("percent").abs() < 1e-9);
    }

    #[test]
    fn cache_reads_use_the_cached_rate() {
        let prices = PriceTable::from([(
            "sol".to_string(),
            ModelPrice {
                input_per_mtok: 1.0,
                cached_input_per_mtok: Some(0.1),
                output_per_mtok: 0.0,
            },
        )]);
        let mut totals = DayTotals::default();
        totals.routed.insert(
            "sol".to_string(),
            ModelTokens {
                requests: 1,
                fresh_input: 1_000_000,
                cached_input: 1_000_000,
                cache_write: 1_000_000,
                output: 0,
            },
        );

        let savings = estimate(&totals, &prices, "sol").expect("priced");

        assert!(
            (savings.actual - 2.1).abs() < 1e-9,
            "cache writes bill at the full input rate, reads at the cached rate"
        );
    }

    #[test]
    fn an_unpriced_model_suppresses_the_estimate() {
        let mut totals = DayTotals::default();
        totals
            .routed
            .insert("unknown".to_string(), tokens(1_000, 100));

        assert!(estimate(&totals, &table(), "sol").is_none());
    }

    #[test]
    fn an_unpriced_baseline_suppresses_the_estimate() {
        let mut totals = DayTotals::default();
        totals.routed.insert("luna".to_string(), tokens(1_000, 100));

        assert!(estimate(&totals, &table(), "absent").is_none());
    }

    #[test]
    fn an_empty_day_has_no_percentage() {
        let savings = estimate(&DayTotals::default(), &table(), "sol").expect("priced");

        assert_eq!(savings.saved(), 0.0);
        assert!(savings.percent().is_none());
    }
}
