#!/usr/bin/env bash
# CI guard: assert the attune-server integration-test split lists in ci.yml
# together cover EVERY tests/*.rs on disk, so sharding can never silently drop a
# test. Run from rust/ (working-directory: rust).
#
# attune-server's 36 integration test files are partitioned across FIVE jobs
# (2026-06-10: split finer — group-A + office each overran the 50min budget on
# the slow runners because every shard recompiles the full release dep tree and
# the server-booting / argon2-vault-setup tests are CPU-bound):
#   - OFFICE_A (5)  runs in rust-test-server-office    (incl. office_concurrent ≈ 99s)
#   - OFFICE_B (5)  runs in rust-test-server-office-b
#   - GROUP_A  (7)  runs in rust-test-server           (+ lib + accounts + root pkg)
#   - GROUP_B  (13) runs in rust-test-server-b         (lighter wire/logic tests)
#   - GROUP_C  (6)  runs in rust-test-server-c         (heavy server-booting / wizard / vault tests)
# This script recomputes the union of all five and compares to on-disk.
set -euo pipefail

OFFICE_A="office_asr_golden_gate office_cancel_test office_concurrent_test \
office_error_contract office_failure_recovery_test"

OFFICE_B="office_happy_path office_ocr_golden_gate office_prop_tests \
office_schema_compat office_six_category_floor"

GROUP_A="ai_stack_web_search_test git_route_subprocess lock_order_abba_test \
marketplace_install_test ocr_profiles_routes_test privacy_endpoints_test \
vault_lock_endpoint_test"

GROUP_B="acp4_governor_wire_test acp5_chat_flow_wire_test amd_laptop_e2e_smoke \
api_v1_version_test chat_cost_estimate_test eval_determinism_test \
eval_response_surface_test forms_routes_test index_path_test lib_runtime_test \
member_routes_test session_test store_queue_test"

GROUP_C="form_factor_integration projects_routes_test settings_lock_test \
system_wizard_full_flow_test vault_recovery_test vault_setup_test"

covered=$(echo "$OFFICE_A $OFFICE_B $GROUP_A $GROUP_B $GROUP_C" | tr ' ' '\n' | grep -v '^$' | sort -u)
ondisk=$(for f in crates/attune-server/tests/*.rs; do basename "$f" .rs; done | sort -u)

if [ "$covered" != "$ondisk" ]; then
  echo "::error::attune-server test split-list drifted from on-disk tests/*.rs."
  echo "--- in split lists but NOT on disk ---"; comm -23 <(echo "$covered") <(echo "$ondisk") || true
  echo "--- on disk but NOT in any split list (UNCOVERED!) ---"; comm -13 <(echo "$covered") <(echo "$ondisk") || true
  echo "Update OFFICE_A/OFFICE_B/GROUP_A/GROUP_B/GROUP_C in ci.yml + this guard so every server test file is assigned to a shard."
  exit 1
fi
echo "server test split-list OK ($(echo "$ondisk" | wc -w) files = $(echo "$OFFICE_A" | wc -w) office-a + $(echo "$OFFICE_B" | wc -w) office-b + $(echo "$GROUP_A" | wc -w) group-A + $(echo "$GROUP_B" | wc -w) group-B + $(echo "$GROUP_C" | wc -w) group-C)."
