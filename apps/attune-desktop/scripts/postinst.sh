#!/bin/sh
#
# attune Linux package post-install hook — edge-scheduler safe host setup.
# (R-deploy / 2026-07-12)
#
# The package installer must not install, start, or tune concrete local
# inference workers. Production AI execution is owned by either:
#   - a user-configured cloud/OpenAI-compatible endpoint, or
#   - an edge scheduler service supplied by the host.
#
# This hook is intentionally best-effort and never fails dpkg/rpm. It only
# prepares Attune-owned directories, removes legacy Attune-managed worker
# shims when safe, and logs the next configuration step.

PATH="/usr/local/bin:/usr/local/sbin:$PATH"
export PATH

LOG_TAG="attune-postinst"
log() { logger -t "$LOG_TAG" -- "$1"; printf '[attune-postinst] %s\n' "$1"; }

if [ "$(uname -s)" != "Linux" ]; then
  log "non-Linux platform; skipping post-install hooks."
  exit 0
fi

target_home() {
  if [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER:-}" != "root" ]; then
    HOME_FROM_PASSWD=$(getent passwd "$SUDO_USER" 2>/dev/null | cut -d: -f6)
    if [ -n "$HOME_FROM_PASSWD" ]; then
      printf '%s\n' "$HOME_FROM_PASSWD"
      return 0
    fi
  fi
  printf '%s\n' "${HOME:-/root}"
}

detect_form_factor() {
  case "${ATTUNE_FORM_FACTOR:-}" in
    local_scheduler|local-scheduler|edge_scheduler|edge-scheduler|appliance)
      printf '%s\n' "edge_scheduler"
      return 0
      ;;
  esac
  if [ -r /sys/class/dmi/id/product_name ]; then
    PROD=$(tr 'A-Z' 'a-z' < /sys/class/dmi/id/product_name 2>/dev/null)
    case "$PROD" in
      *local-scheduler*|*edge-scheduler*|*attune-appliance*)
        printf '%s\n' "edge_scheduler"
        return 0
        ;;
    esac
  fi
  printf '%s\n' "generic"
}

normalize_scheduler_base() {
  printf '%s' "$1" | sed 's:/*$::; s:/v1$::'
}

probe_scheduler() {
  BASE=$(normalize_scheduler_base "$1")
  if [ -z "$BASE" ]; then
    return 1
  fi
  if ! command -v curl >/dev/null 2>&1; then
    log "edge scheduler configured but curl is missing; skipping post-install probe: $BASE"
    return 1
  fi
  if curl -fsS --max-time 3 "$BASE/ready?hot=1" >/dev/null 2>&1 \
     || curl -fsS --max-time 3 "$BASE/ready" >/dev/null 2>&1 \
     || curl -fsS --max-time 3 "$BASE/health" >/dev/null 2>&1 \
     || curl -fsS --max-time 3 "$BASE/healthz" >/dev/null 2>&1; then
    log "edge scheduler reachable: $BASE"
    return 0
  fi
  log "WARN: edge scheduler not reachable during post-install probe: $BASE"
  return 1
}

HOME_DIR=$(target_home)
ATTUNE_DATA_ROOT="$HOME_DIR/.local/share/attune"
ATTUNE_CONFIG_ROOT="$HOME_DIR/.config/npu-vault"

mkdir -p "$ATTUNE_DATA_ROOT/logs" "$ATTUNE_DATA_ROOT/models" "$ATTUNE_CONFIG_ROOT" 2>/dev/null || true
if [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER:-}" != "root" ]; then
  chown -R "$SUDO_USER:$SUDO_USER" "$ATTUNE_DATA_ROOT" "$ATTUNE_CONFIG_ROOT" 2>/dev/null || true
fi
log "prepared Attune data directories under $HOME_DIR"

# Remove only legacy shims that previous Attune packages created. We do not
# uninstall or restart third-party runtimes.
DROPIN=/etc/systemd/system/ollama.service.d/hsa-override.conf
if [ -f "$DROPIN" ] && grep -q 'attune-desktop postinst' "$DROPIN" 2>/dev/null; then
  rm -f "$DROPIN" 2>/dev/null || true
  rmdir /etc/systemd/system/ollama.service.d 2>/dev/null || true
  systemctl daemon-reload >/dev/null 2>&1 || true
  log "removed legacy Attune-managed Ollama HSA drop-in; Ollama service was not restarted"
fi

WHISPER_LINK="/usr/local/bin/whisper-cli"
if [ -L "$WHISPER_LINK" ]; then
  TARGET=$(readlink "$WHISPER_LINK" 2>/dev/null || true)
  case "$TARGET" in
    /usr/lib/Attune/bin/whisper-cli|/usr/lib/attune/bin/whisper-cli)
      rm -f "$WHISPER_LINK" 2>/dev/null || true
      log "removed legacy Attune-created whisper-cli system symlink"
      ;;
  esac
fi

FORM_FACTOR=$(detect_form_factor)
SCHEDULER_URL="${ATTUNE_EDGE_SCHEDULER_URL:-${ATTUNE_LOCAL_SCHEDULER_BASE:-}}"
log "form factor: $FORM_FACTOR"

if [ -n "$SCHEDULER_URL" ]; then
  probe_scheduler "$SCHEDULER_URL" || true
else
  log "no edge scheduler URL configured; first-run wizard should configure cloud LLM or an edge scheduler endpoint"
fi

if command -v ollama >/dev/null 2>&1; then
  log "legacy/self-managed Ollama detected but not managed by Attune: $(ollama --version 2>&1 | head -1)"
else
  log "Ollama not installed; this is expected for the scheduler/cloud default path"
fi

log "AI runtimes and model weights are not installed by the package hook."
log "Embedding/rerank/OCR/ASR/LLM acceleration must come through the edge scheduler, or cloud/BYOK settings for LLM."
log "post-install complete."
exit 0
