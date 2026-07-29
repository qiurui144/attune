use attune_core::platform::FormFactor;
use attune_core::retrieval_plan::{self, RetrievalPlan};
use attune_core::search::SearchParams;
use attune_core::store::audit::PrivacyTier;

use crate::eval::ParsedEvalHeaders;

#[allow(clippy::too_many_arguments)] // Public route glue passes the existing retrieval knobs explicitly.
pub(crate) fn build_search_params(
    form_factor: FormFactor,
    use_local_scheduler_profile: bool,
    rerank_enabled: bool,
    query: &str,
    detected_domain: Option<&str>,
    top_k: usize,
    initial_k: Option<usize>,
    intermediate_k: Option<usize>,
    eval: Option<&ParsedEvalHeaders>,
) -> (SearchParams, Option<RetrievalPlan>) {
    let mut retrieval_plan = None;
    let use_edge_profile = form_factor.prefers_local_llm() || use_local_scheduler_profile;
    let mut params = if use_edge_profile {
        let mut req = retrieval_plan::RetrievalPlanRequest::local_scheduler_interactive(query);
        if !form_factor.prefers_local_llm() {
            req.target = retrieval_plan::RetrievalTarget::LocalWorkstation;
        }
        req.top_k = top_k;
        req.privacy_tier = PrivacyTier::L0;
        req.corpus_domain = detected_domain;
        let plan = retrieval_plan::plan_retrieval(req);
        let plan_params = plan.to_search_params();
        retrieval_plan = Some(plan);
        plan_params
    } else {
        let mut p = SearchParams::with_defaults(top_k);
        if let Some(d) = detected_domain {
            p.domain_hint = Some(d.to_string());
        }
        p
    };

    if use_edge_profile {
        if let Some(ik) = initial_k {
            params.initial_k = ik.max(params.top_k).min(params.initial_k);
        }
        if let Some(imk) = intermediate_k {
            params.intermediate_k = imk.max(params.top_k).min(params.intermediate_k);
        }
    } else {
        if let Some(ik) = initial_k {
            params.initial_k = ik;
        }
        if let Some(imk) = intermediate_k {
            params.intermediate_k = imk;
        }
    }

    let rerank_enabled = rerank_enabled
        || env_bool_any(
            &[
                "ATTUNE_RERANK_ENABLED",
                "ATTUNE_SCHEDULER_RERANK_ENABLED",
                "ATTUNE_LOCAL_RERANK_ENABLED",
            ],
            false,
        );
    if use_edge_profile && !rerank_enabled {
        params.skip_rerank = true;
    }
    if let Some(eval) = eval {
        params.seed = eval.seed;
        params.skip_rewrite = eval.skip_rewrite;
        if eval.skip_rerank {
            params.skip_rerank = true;
        }
    }
    if env_bool_any(
        &[
            "ATTUNE_RERANK_DISABLED",
            "ATTUNE_SCHEDULER_RERANK_DISABLED",
            "ATTUNE_LOCAL_RERANK_DISABLED",
        ],
        false,
    ) {
        params.skip_rerank = true;
    }

    (params, retrieval_plan)
}

pub(crate) fn rerank_enabled_from_settings(settings: Option<&serde_json::Value>) -> bool {
    settings
        .and_then(|settings| settings.get("rerank"))
        .and_then(|rerank| rerank.get("enabled"))
        .and_then(|enabled| enabled.as_bool())
        .unwrap_or(false)
}

fn env_bool_any(keys: &[&str], default: bool) -> bool {
    keys.iter()
        .find_map(|key| {
            std::env::var(key).ok().map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
        })
        .unwrap_or(default)
}

pub(crate) fn retrieval_plan_trace(plan: Option<&RetrievalPlan>) -> Option<serde_json::Value> {
    let plan = plan?;
    Some(serde_json::json!({
        "target": format!("{:?}", plan.target),
        "latency_class": format!("{:?}", plan.latency_class),
        "final_top_k": plan.final_top_k,
        "rerank_candidate_cap": plan.rerank_candidate_cap,
        "evidence_token_budget": plan.evidence_token_budget,
        "local_only": plan.partitions.local_only,
        "shard_key": plan.partitions.shard_key,
        "domain_hint": plan.domain_hint,
        "channels": plan.channels.iter().map(|channel| serde_json::json!({
            "channel": format!("{:?}", channel.channel),
            "top_k": channel.top_k,
            "weight": channel.weight,
            "min_score": channel.min_score,
            "required": channel.required,
        })).collect::<Vec<_>>(),
        "partitions": plan.partitions.filters.iter().map(|filter| serde_json::json!({
            "field": format!("{:?}", filter.field),
            "value": filter.value,
            "required": filter.required,
        })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_core::retrieval_plan::RetrievalTarget;

    #[test]
    fn local_scheduler_search_params_use_retrieval_planner_caps() {
        let eval = ParsedEvalHeaders {
            seed: Some(42),
            skip_rewrite: true,
            skip_rerank: true,
            ..Default::default()
        };

        let (params, plan) = build_search_params(
            FormFactor::LocalSchedulerAppliance,
            false,
            true,
            "ACME-2026-001 合同条款",
            Some("legal"),
            100,
            Some(500),
            Some(500),
            Some(&eval),
        );
        let plan = plan.expect("local scheduler search should produce a retrieval plan");

        assert_eq!(plan.target, RetrievalTarget::LocalScheduler);
        assert!(plan.partitions.local_only);
        assert_eq!(params.top_k, 8);
        assert!(params.initial_k <= 160);
        assert_eq!(params.intermediate_k, 20);
        assert_eq!(params.domain_hint.as_deref(), Some("legal"));
        assert_eq!(params.seed, Some(42));
        assert!(params.skip_rewrite);
        assert!(params.skip_rerank);
    }

    #[test]
    fn non_scheduler_search_params_keep_legacy_overrides() {
        let (params, plan) = build_search_params(
            FormFactor::Laptop,
            false,
            false,
            "ordinary query",
            Some("tech"),
            12,
            Some(77),
            Some(33),
            None,
        );

        assert!(plan.is_none());
        assert_eq!(params.top_k, 12);
        assert_eq!(params.initial_k, 77);
        assert_eq!(params.intermediate_k, 33);
        assert_eq!(params.domain_hint.as_deref(), Some("tech"));
        assert_eq!(params.min_score, None);
    }

    #[test]
    fn local_scheduler_search_params_use_edge_profile_without_scheduler_form_factor() {
        let (params, plan) = build_search_params(
            FormFactor::Server,
            true,
            false,
            "A320 QRH abnormal procedure",
            Some("aviation"),
            50,
            Some(500),
            Some(500),
            None,
        );
        let plan = plan.expect("local scheduler search should produce a retrieval plan");

        assert_eq!(plan.target, RetrievalTarget::LocalWorkstation);
        assert!(plan.partitions.local_only);
        assert_eq!(params.top_k, 12);
        assert!(params.initial_k <= 200);
        assert_eq!(params.intermediate_k, 40);
        assert_eq!(params.domain_hint.as_deref(), Some("aviation"));
        assert!(params.skip_rerank);
    }

    #[test]
    fn local_scheduler_search_params_honor_settings_rerank_enabled() {
        let (params, plan) = build_search_params(
            FormFactor::Server,
            true,
            true,
            "TCP/IP troubleshooting workflow",
            None,
            20,
            None,
            None,
            None,
        );

        assert!(plan.is_some());
        assert!(!params.skip_rerank);
    }

    #[test]
    fn rerank_enabled_from_settings_uses_rerank_flag_only() {
        assert!(rerank_enabled_from_settings(Some(&serde_json::json!({
            "rerank": {"enabled": true}
        }))));
        assert!(!rerank_enabled_from_settings(Some(&serde_json::json!({
            "rerank": {"enabled": false}
        }))));
        assert!(!rerank_enabled_from_settings(Some(&serde_json::json!({}))));
    }
}
