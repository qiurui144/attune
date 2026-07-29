#!/usr/bin/env bash
# Run the OSS agent quality gates recorded in agent_quality_manifest.yaml.
#
# This is an offline deterministic/stability lane. It intentionally excludes
# real-LLM, OCR, ASR, and external-service tests.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo "==> OSS deterministic agent gates"
cargo test -p attune-core \
  --test agent_gate_orchestrator \
  --test document_classifier_agent_golden_gate \
  --test linker_golden_gate \
  --test chat_reliability_golden_gate \
  --test memory_consolidation_agent_golden_gate \
  --test self_evolving_skill_agent_golden_gate \
  --quiet

echo "==> OSS companion/integration/proptest gates"
cargo test -p attune-core \
  --test document_classifier_agent_integration \
  --test document_classifier_agent_proptests \
  --test chat_reliability_proptests \
  --test memory_consolidation_agent_integration \
  --test memory_consolidation_agent_proptests \
  --test self_evolving_skill_agent_integration \
  --test self_evolving_skill_agent_proptests \
  --quiet

echo "==> OSS agent quality gates passed"
