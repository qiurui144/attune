#!/usr/bin/env bash
# CI guard: assert the attune-core integration-test split lists in ci.yml together
# cover EVERY non-#[ignore]-irrelevant test file on disk, so sharding can never
# silently drop a test. Run from rust/ (working-directory: rust).
#
# The core integration test files are partitioned across three places:
#   - 3 deterministic gates run as dedicated ci.yml steps (GATES below)
#   - HALF_A runs in rust-test-core           (the --test list there)
#   - HALF_B runs in rust-test-core-b         (the --test list there)
# This script recomputes HALF_A ∪ HALF_B ∪ GATES and compares to the on-disk set.
set -euo pipefail

GATES="parse_golden_set_regression agent_gate_orchestrator wasm_capability_gate"

HALF_A="asr_ingest_test change_password_test chat_reliability_golden_gate \
chat_reliability_integration chat_reliability_proptests chunking_quality_test \
concurrent_stress_test crash_recovery_test deepsum_savings \
doc_compare_verdict_golden_gate doc_intel_real_llm_gate \
document_classifier_agent_golden_gate \
document_classifier_agent_integration document_classifier_agent_proptests \
email_accounts_test entities_test generic_plugins_test git_connector \
governor_integration i18n_ingest_search_test ingest_edge_resource_test \
ingest_email_test ingest_pipeline_test ingest_rss_test linker_entity_debug \
linker_golden_gate memory_consolidation_agent_golden_gate \
memory_consolidation_agent_integration memory_consolidation_agent_proptests \
memory_consolidation_integration memory_moat_integration \
memory_token_reduction_benchmark migration_roundtrip_test model_boundary_audit \
multilayer_memory_integration"

HALF_B="job_queue_durable nontext_cross_validate_golden \
ocr_image_test ocr_long_page_audit office_adversarial_test \
office_formats_test oom_behavior_test oss_agent_real_llm_gate oss_boundary_test \
pdf_e2e_search pdf_ingest_test perf_chunker_bench perf_reindex_bench \
pii_chat_path_redact_test plugin_protocol_e2e ppocr_icbc_smoke \
project_recommender_test rag_flow_audit rag_perf_audit rag_quality_benchmark \
rag_w2_batch1_integration rag_w3_batch_a_integration rag_w3_batch_b_integration \
reranker_long_doc_audit retrieval_quality_test rss_feeds_test \
self_evolving_skill_agent_golden_gate self_evolving_skill_agent_integration \
self_evolving_skill_agent_proptests session_revoke_test stress_large_scale_test \
webdav_remotes_test workflow_test"

covered=$(echo "$GATES $HALF_A $HALF_B" | tr ' ' '\n' | grep -v '^$' | sort -u)
ondisk=$(for f in crates/attune-core/tests/*.rs; do basename "$f" .rs; done | sort -u)

if [ "$covered" != "$ondisk" ]; then
  echo "::error::attune-core test split-list drifted from on-disk tests/*.rs."
  echo "--- in split lists but NOT on disk ---"; comm -23 <(echo "$covered") <(echo "$ondisk") || true
  echo "--- on disk but NOT in any split list (UNCOVERED!) ---"; comm -13 <(echo "$covered") <(echo "$ondisk") || true
  echo "Update HALF_A/HALF_B in ci.yml + this guard so every core test file is assigned to a shard."
  exit 1
fi
echo "core test split-list OK ($(echo "$ondisk" | wc -w) files = $(echo "$GATES" | wc -w) gates + $(echo "$HALF_A" | wc -w) half-A + $(echo "$HALF_B" | wc -w) half-B)."
