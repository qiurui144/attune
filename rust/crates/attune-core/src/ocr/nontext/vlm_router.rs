//! I3 — VLM model-matrix failover (spec 2026-06-16 §I3 / §3.2 / §5.3).
//!
//! Stage 4 escalation used to hold a SINGLE `&dyn VlmProvider`: when that provider was
//! unreachable / 5xx / timed out / rate-limited / lacked vision capability, the whole
//! escalation degraded straight to local (spec §1.2 reliability gap). This module turns the
//! single provider into a **prioritized candidate matrix with health probing + failover**:
//!
//!   primary (highest priority, healthy) → on provider failure → next candidate → ... →
//!   all candidates exhausted → degrade to local (the existing §7 path, unchanged).
//!
//! ## What failover is and is NOT (cost contract, spec §8)
//!
//! Failover is **sequential, not fan-out**: one candidate is tried at a time; the next is only
//! reached if the current one FAILS at the provider level (not on a parse/grounding retry — those
//! stay inside `escalate_region`'s own ≤3 retry loop on the SAME model). A healthy primary that
//! returns a (possibly ungrounded) result is final — no second model is paid for. So the typical
//! cost stays "1 region → 1 VLM call"; failover adds at most one extra call per dead candidate.
//!
//! ## Egress stays gated
//!
//! The router does NOT bypass the egress gate. The caller mints ONE [`VlmEgressToken`] (per
//! `gate_vlm_egress`, which redacts + downscales + runs the outbound gate) and hands it to
//! [`VlmRouter::escalate`]; every candidate that is actually called receives that same gated token.
//! There is no way to reach a candidate without the token (the token is required by value), so the
//! "L0 never reaches the cloud VLM" invariant (MEMORY P0) holds for every candidate.
//!
//! ## Cache isolation
//!
//! Because the result is keyed downstream by `vlm_model` (telemetry carries the model that actually
//! produced the answer), a failover to a different candidate is correctly attributed — a candidate's
//! answer is never mis-cached under another model's name (spec §3.3).

use super::vlm_escalate::{escalate_region, RegionGeom, VlmEgressToken, VlmTelemetry};
use super::RegionKind;
use crate::error::{Result, VaultError};
use crate::vlm::VlmProvider;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One model in the failover matrix (spec §5.3). Higher `priority` is tried first.
pub struct VlmCandidate {
    /// The provider that speaks to this model.
    pub provider: Arc<dyn VlmProvider>,
    /// The model identifier (telemetry key + failover-candidate key, e.g. `"qwen-3.7-max"`).
    pub model: String,
    /// Failover order; the router tries the highest priority healthy candidate first.
    pub priority: u8,
}

impl VlmCandidate {
    pub fn new(provider: Arc<dyn VlmProvider>, model: impl Into<String>, priority: u8) -> Self {
        Self {
            provider,
            model: model.into(),
            priority,
        }
    }
}

/// Why a candidate was skipped/failed — drives the `kind × model` failure aggregation (§I4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailKind {
    /// `probe()` reported the candidate unavailable (skipped before any call).
    Unhealthy,
    /// The provider call itself errored (5xx / timeout / rate-limit / unreachable / capability).
    Provider,
}

/// The 💰-tier model matrix: a prioritized set of candidates with health probing + failover
/// (spec §I3). Construction is order-independent; candidates are sorted by descending priority on
/// `select`. The matrix is shared (`Arc`-friendly) and its failure aggregator is interior-mutable
/// behind its OWN `Mutex` so it never participates in the search/index lock order (R6: probe + record
/// use only this mutex, never held across `escalate_region`).
pub struct VlmRouter {
    candidates: Vec<VlmCandidate>,
    /// `(region_kind, model)` → (total, failures). Spec §I4 §4.5-F: rate > 30% → UI hint.
    failures: Mutex<HashMap<(RegionKind, String), (u64, u64)>>,
}

/// The §4.5-F red line shared with `agent_telemetry::FAILURE_RATE_ALERT_THRESHOLD`: a per
/// `(kind × model)` failure rate strictly above this triggers the "switch to a higher tier" hint.
pub const VLM_FAILURE_RATE_ALERT_THRESHOLD: f64 =
    crate::agent_telemetry::FAILURE_RATE_ALERT_THRESHOLD;

impl VlmRouter {
    /// Build a router from candidates (any order). An empty matrix is valid — `escalate` then
    /// degrades to local immediately (no candidate to try), matching the §7 "all candidates failed"
    /// path so a misconfigured deployment never panics.
    pub fn new(candidates: Vec<VlmCandidate>) -> Self {
        Self {
            candidates,
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// Single-provider convenience: a 1-candidate matrix (back-compat with the old single
    /// `state.vlm()` shape — a router that wraps exactly one provider behaves like the pre-failover
    /// code, just with telemetry aggregation).
    pub fn single(provider: Arc<dyn VlmProvider>) -> Self {
        let model = provider.vlm_model_name().to_string();
        Self::new(vec![VlmCandidate::new(provider, model, 9)])
    }

    /// Candidates ordered for failover: descending priority, healthy ones only (probe == true).
    /// Unhealthy candidates are recorded as `FailKind::Unhealthy` against the requested `kind` so a
    /// chronically-down primary surfaces in the §I4 aggregation even when failover hides it from the
    /// user (spec R2: failover must not mask a dead primary).
    fn healthy_ordered(&self, kind: RegionKind) -> Vec<&VlmCandidate> {
        let mut ordered: Vec<&VlmCandidate> = self.candidates.iter().collect();
        ordered.sort_by_key(|c| std::cmp::Reverse(c.priority));
        ordered
            .into_iter()
            .filter(|c| {
                if c.provider.probe() {
                    true
                } else {
                    self.record(kind, &c.model, FailKind::Unhealthy);
                    false
                }
            })
            .collect()
    }

    /// Escalate one region through the matrix with failover (spec §I3 data-flow). The egress token
    /// (already gated: redacted + downscaled) is consumed by the first candidate that actually runs;
    /// because `escalate_region` takes the token by reference here, every tried candidate reuses the
    /// SAME gated bytes (no re-gating, no extra egress). On a provider error the next healthy
    /// candidate is tried; when every candidate has failed the result is `Err` and the caller
    /// degrades to local (spec §7), with `tried` listing what was attempted (for the warning).
    ///
    /// Returns the winning candidate's `(Result, VlmTelemetry)` PLUS the ordered list of models
    /// tried (so the caller can surface "已切换备用模型" / record per-candidate failures).
    pub fn escalate(
        &self,
        token: &VlmEgressToken,
        kind: RegionKind,
        geom: RegionGeom,
    ) -> VlmRouteOutcome {
        let healthy = self.healthy_ordered(kind);
        let mut tried: Vec<String> = Vec::new();
        for cand in &healthy {
            tried.push(cand.model.clone());
            let (res, tel) =
                escalate_region(cand.provider.as_ref(), token, kind, &cand.model, geom);
            match &res {
                // A parsed result (grounded OR kept-ungrounded) is a SUCCESS for failover purposes:
                // the model produced an answer. error_kind == Some("provider") is the ONLY signal to
                // fail over; "grounding"/"parse" are this-model outcomes already exhausted ≤3 inside
                // escalate_region — switching model on them would be paying twice for the same call.
                Ok(_) => {
                    self.record(kind, &cand.model, /*failed=*/ None);
                    return VlmRouteOutcome {
                        result: res,
                        telemetry: tel,
                        tried,
                    };
                }
                Err(_) => {
                    self.record(kind, &cand.model, Some(FailKind::Provider));
                    // try next candidate
                }
            }
        }
        // Every healthy candidate failed (or there were none): degrade to local (spec §7). The
        // caller maps this Err to `vlm_unavailable` + keeps the local result; NEVER panics.
        VlmRouteOutcome {
            result: Err(VaultError::Io(std::io::Error::other(
                "vlm-router: all candidates failed (degrade to local)",
            ))),
            telemetry: VlmTelemetry {
                region_kind: kind,
                vlm_model: tried.last().cloned().unwrap_or_else(|| "none".into()),
                error_kind: Some("all-candidates-failed".into()),
                retry_count: 0,
            },
            tried,
        }
    }

    /// Record one outcome into the `(kind × model)` aggregator (§I4). `failed = None` is a success;
    /// `Some(_)` is a failure (the FailKind variant is informational — both Unhealthy and Provider
    /// count toward the failure rate, since both mean "the user did not get this model's answer").
    fn record(&self, kind: RegionKind, model: &str, failed: impl Into<Option<FailKind>>) {
        let failed = failed.into();
        let mut g = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let entry = g.entry((kind, model.to_string())).or_insert((0, 0));
        entry.0 += 1;
        if failed.is_some() {
            entry.1 += 1;
        }
    }

    /// Failure rate for one `(kind, model)` — `failures / total`, `0.0` for no calls (§9 boundary:
    /// never divides by zero).
    pub fn failure_rate(&self, kind: RegionKind, model: &str) -> f64 {
        let g = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        match g.get(&(kind, model.to_string())) {
            Some(&(total, fails)) if total > 0 => fails as f64 / total as f64,
            _ => 0.0,
        }
    }

    /// §4.5-F: should the UI suggest switching to a higher tier for this `(kind, model)`?
    /// True when the failure rate is strictly above [`VLM_FAILURE_RATE_ALERT_THRESHOLD`].
    pub fn should_suggest_higher_tier(&self, kind: RegionKind, model: &str) -> bool {
        self.failure_rate(kind, model) > VLM_FAILURE_RATE_ALERT_THRESHOLD
    }

    /// Snapshot of every `(kind, model)` failure rate (for the REST `vlm_hint.kind_failure_rates`
    /// surface, spec §5.1). Keys are `"<kind>:<model>"` to match the wire format.
    pub fn failure_rate_snapshot(&self) -> HashMap<String, f64> {
        let g = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        g.iter()
            .filter(|(_, &(total, _))| total > 0)
            .map(|((kind, model), &(total, fails))| {
                (
                    format!("{kind:?}:{model}").to_lowercase(),
                    fails as f64 / total as f64,
                )
            })
            .collect()
    }

    /// True if ANY `(kind, model)` is above threshold — drives the top-level `vlm_hint
    /// .suggest_higher_tier` boolean (spec §5.1 / §I4).
    pub fn any_suggest_higher_tier(&self) -> bool {
        let g = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        g.values()
            .any(|&(total, fails)| total > 0 && (fails as f64 / total as f64) > VLM_FAILURE_RATE_ALERT_THRESHOLD)
    }

    /// Number of registered candidates (0 = degrade-only matrix).
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }
}

/// The result of one routed escalation: the winning (or final-failure) `Result` + telemetry, plus
/// the ordered list of models that were tried (empty only if the matrix had no healthy candidate).
pub struct VlmRouteOutcome {
    pub result: Result<super::RegionResult>,
    pub telemetry: VlmTelemetry,
    pub tried: Vec<String>,
}

impl VlmRouteOutcome {
    /// True when more than one candidate was tried — i.e. a failover actually happened (the caller
    /// surfaces "已切换备用模型" in the UI, spec §8.1.5).
    pub fn failed_over(&self) -> bool {
        self.tried.len() > 1
    }

    /// The model that produced the returned result (or the last one tried on total failure).
    pub fn winning_model(&self) -> &str {
        &self.telemetry.vlm_model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::BBox;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A scripted VLM: each `vqa` returns the next answer (or errors). Counts calls so tests can
    /// assert how many candidates were actually paid for (cost contract: failover ≠ fan-out).
    struct ScriptVlm {
        model: String,
        /// `Ok(json)` answers (consumed in order). When exhausted, returns the `default`.
        answers: Mutex<Vec<std::result::Result<String, ()>>>,
        default: std::result::Result<String, ()>,
        healthy: bool,
        calls: AtomicUsize,
    }

    impl ScriptVlm {
        fn new(model: &str, answers: Vec<std::result::Result<String, ()>>, healthy: bool) -> Self {
            Self {
                model: model.into(),
                answers: Mutex::new(answers),
                default: Err(()),
                healthy,
                calls: AtomicUsize::new(0),
            }
        }
        fn always_ok(model: &str, json: &str) -> Self {
            Self {
                model: model.into(),
                answers: Mutex::new(vec![]),
                default: Ok(json.into()),
                healthy: true,
                calls: AtomicUsize::new(0),
            }
        }
        fn always_err(model: &str, healthy: bool) -> Self {
            Self {
                model: model.into(),
                answers: Mutex::new(vec![]),
                default: Err(()),
                healthy,
                calls: AtomicUsize::new(0),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl VlmProvider for ScriptVlm {
        fn caption(&self, _p: &Path) -> Result<String> {
            Ok(String::new())
        }
        fn vqa(&self, _p: &Path, _q: &str) -> Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut a = self.answers.lock().unwrap();
            let next = if a.is_empty() {
                self.default.clone()
            } else {
                a.remove(0)
            };
            next.map_err(|()| VaultError::Io(std::io::Error::other("scripted provider error")))
        }
        fn probe(&self) -> bool {
            self.healthy
        }
        fn vlm_model_name(&self) -> &str {
            &self.model
        }
    }

    /// A grounded handwriting answer the grounding validator accepts (sub_bbox inside the crop).
    fn good_handwriting() -> String {
        r#"{"text":"hello","grounding":{"sub_bbox":{"x":1,"y":1,"w":10,"h":10}}}"#.into()
    }

    fn geom() -> RegionGeom {
        RegionGeom {
            page_bbox: BBox { x: 0, y: 0, w: 100, h: 100 },
            page: 0,
            crop_w: 100,
            crop_h: 100,
        }
    }

    // A token can only be minted by the gate; tests reach into the test-only constructor exposed by
    // the vlm_escalate module's `#[cfg(test)]` helper.
    fn test_token() -> VlmEgressToken {
        super::super::vlm_escalate::test_egress_token()
    }

    #[test]
    fn primary_healthy_returns_without_failover() {
        let primary = Arc::new(ScriptVlm::always_ok("qwen-3.7-max", &good_handwriting()));
        let backup = Arc::new(ScriptVlm::always_ok("qwen-3.6-flash", &good_handwriting()));
        let router = VlmRouter::new(vec![
            VlmCandidate::new(primary.clone(), "qwen-3.7-max", 9),
            VlmCandidate::new(backup.clone(), "qwen-3.6-flash", 5),
        ]);
        let out = router.escalate(&test_token(), RegionKind::Handwriting, geom());
        assert!(out.result.is_ok());
        assert!(!out.failed_over(), "healthy primary must not fail over");
        assert_eq!(out.winning_model(), "qwen-3.7-max");
        assert_eq!(primary.call_count(), 1, "primary called once");
        assert_eq!(backup.call_count(), 0, "backup never paid for (no fan-out)");
    }

    #[test]
    fn primary_provider_error_falls_over_to_backup() {
        let primary = Arc::new(ScriptVlm::always_err("qwen-3.7-max", /*healthy*/ true));
        let backup = Arc::new(ScriptVlm::always_ok("qwen-3.6-flash", &good_handwriting()));
        let router = VlmRouter::new(vec![
            VlmCandidate::new(primary.clone(), "qwen-3.7-max", 9),
            VlmCandidate::new(backup.clone(), "qwen-3.6-flash", 5),
        ]);
        let out = router.escalate(&test_token(), RegionKind::Handwriting, geom());
        assert!(out.result.is_ok(), "backup produced a valid result");
        assert!(out.failed_over(), "a failover happened");
        assert_eq!(out.winning_model(), "qwen-3.6-flash");
        assert_eq!(out.tried, vec!["qwen-3.7-max", "qwen-3.6-flash"]);
        // Aggregator: primary has a failure, backup a success.
        assert!(router.failure_rate(RegionKind::Handwriting, "qwen-3.7-max") > 0.0);
        assert_eq!(router.failure_rate(RegionKind::Handwriting, "qwen-3.6-flash"), 0.0);
    }

    #[test]
    fn unhealthy_primary_is_skipped_by_probe() {
        // primary probe == false → skipped WITHOUT a call; backup serves.
        let primary = Arc::new(ScriptVlm::new(
            "qwen-3.7-max",
            vec![Ok(good_handwriting())],
            /*healthy*/ false,
        ));
        let backup = Arc::new(ScriptVlm::always_ok("qwen-3.6-flash", &good_handwriting()));
        let router = VlmRouter::new(vec![
            VlmCandidate::new(primary.clone(), "qwen-3.7-max", 9),
            VlmCandidate::new(backup.clone(), "qwen-3.6-flash", 5),
        ]);
        let out = router.escalate(&test_token(), RegionKind::Handwriting, geom());
        assert!(out.result.is_ok());
        assert_eq!(out.winning_model(), "qwen-3.6-flash");
        assert_eq!(primary.call_count(), 0, "unhealthy primary never called");
        // R2: the dead primary still shows up in the aggregation (Unhealthy counts as a failure).
        assert!(router.failure_rate(RegionKind::Handwriting, "qwen-3.7-max") > 0.0);
    }

    #[test]
    fn all_candidates_fail_degrades_to_local() {
        let a = Arc::new(ScriptVlm::always_err("qwen-3.7-max", true));
        let b = Arc::new(ScriptVlm::always_err("qwen-3.6-flash", true));
        let router = VlmRouter::new(vec![
            VlmCandidate::new(a, "qwen-3.7-max", 9),
            VlmCandidate::new(b, "qwen-3.6-flash", 5),
        ]);
        let out = router.escalate(&test_token(), RegionKind::Handwriting, geom());
        assert!(out.result.is_err(), "all failed → Err (caller degrades to local)");
        assert_eq!(out.telemetry.error_kind.as_deref(), Some("all-candidates-failed"));
        assert_eq!(out.tried.len(), 2, "both candidates were tried");
    }

    #[test]
    fn empty_matrix_degrades_without_panic() {
        let router = VlmRouter::new(vec![]);
        assert_eq!(router.candidate_count(), 0);
        let out = router.escalate(&test_token(), RegionKind::Handwriting, geom());
        assert!(out.result.is_err());
        assert!(out.tried.is_empty());
    }

    #[test]
    fn failure_rate_threshold_and_hint() {
        // 4 calls: 3 fail (provider err), 1 ok → rate 0.75 > 0.30 → suggest higher tier.
        let weak = Arc::new(ScriptVlm::new(
            "qwen-3.6-flash",
            vec![Err(()), Err(()), Err(()), Ok(good_handwriting())],
            true,
        ));
        let router = VlmRouter::single(weak);
        for _ in 0..4 {
            let _ = router.escalate(&test_token(), RegionKind::Handwriting, geom());
        }
        let rate = router.failure_rate(RegionKind::Handwriting, "qwen-3.6-flash");
        assert!((rate - 0.75).abs() < 1e-9, "rate={rate}");
        assert!(router.should_suggest_higher_tier(RegionKind::Handwriting, "qwen-3.6-flash"));
        assert!(router.any_suggest_higher_tier());
        let snap = router.failure_rate_snapshot();
        assert!(snap.contains_key("handwriting:qwen-3.6-flash"));
    }

    #[test]
    fn single_helper_wraps_one_provider() {
        let p = Arc::new(ScriptVlm::always_ok("solo-model", &good_handwriting()));
        let router = VlmRouter::single(p);
        assert_eq!(router.candidate_count(), 1);
        let out = router.escalate(&test_token(), RegionKind::Handwriting, geom());
        assert!(out.result.is_ok());
        assert_eq!(out.winning_model(), "solo-model");
        assert!(!out.failed_over());
    }
}
