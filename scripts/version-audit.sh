#!/usr/bin/env bash
# version-audit.sh — Cross-repo 版本管理审计
#
# 验证 attune / attune-pro / attune-pluginhub / cloud 四仓:
#   - tag 总数 + annotated 比例(应 100% annotated)
#   - main/master HEAD 与最新 tag 是否对齐
#   - attune ↔ attune-pro 版本配对是否同步
#   - RELEASE.md 是否覆盖到最新 tag
#   - lightweight tag 立即报警
#
# 退出码:0 健康 / 1 有警告 / 2 有错误
# 见 docs/VERSIONING.md 第 8 节(周审计)

set -uo pipefail

ATTUNE="${ATTUNE_REPO:-/data/company/project/attune}"
ATTUNE_PRO="${ATTUNE_PRO_REPO:-/data/company/project/attune-pro}"
PLUGINHUB="${PLUGINHUB_REPO:-/data/company/project/attune-pluginhub}"
CLOUD="${CLOUD_REPO:-/data/company/cloud}"

EXIT=0
WARN=0

# ── helpers ──────────────────────────────────────────────────────────────────

red()   { printf "\033[31m%s\033[0m" "$*"; }
green() { printf "\033[32m%s\033[0m" "$*"; }
yellow(){ printf "\033[33m%s\033[0m" "$*"; }
bold()  { printf "\033[1m%s\033[0m" "$*"; }

err()  { red   "ERR " ; echo " $*"; EXIT=2; }
warn() { yellow "WARN"; echo " $*"; WARN=1; }
ok()   { green "OK  " ; echo " $*"; }

audit_repo() {
  local label="$1" dir="$2" main_br="$3"
  echo
  bold "════════ $label ($dir) ════════"; echo
  if [ ! -d "$dir/.git" ]; then err "$label: $dir 不是 git 仓"; return; fi
  cd "$dir" || { err "$label: 无法 cd"; return; }

  # tag 总数 / annotated 比
  local total ann lw
  total=$(git tag | wc -l)
  ann=$(git for-each-ref refs/tags --format='%(objecttype)' | grep -c '^tag$' || true)
  lw=$((total - ann))
  printf "  tags: total=%d  annotated=%d  lightweight=%d\n" "$total" "$ann" "$lw"
  if [ "$lw" -gt 0 ]; then
    err "$label: 有 $lw 个 lightweight tag(违反 VERSIONING §10)"
    git for-each-ref refs/tags --format='%(objecttype) %(refname:short)' | awk '$1=="commit"{print "    "$2}'
  fi

  # main/master HEAD 与最新 tag 对齐
  if git rev-parse --verify "$main_br" >/dev/null 2>&1; then
    if [ "$total" -gt 0 ]; then
      local last_tag
      last_tag=$(git describe --tags --abbrev=0 "$main_br" 2>/dev/null || echo "")
      if [ -n "$last_tag" ]; then
        local ahead
        ahead=$(git rev-list --count "$last_tag..$main_br")
        if [ "$ahead" -eq 0 ]; then
          ok "$main_br HEAD == 最新 tag ($last_tag)"
        else
          warn "$main_br 比最新 tag ($last_tag) 多 $ahead 个 commit(可能是治理对齐 merge,确认无需 tag)"
        fi
      fi
    else
      warn "$label: 0 tag(待 backfill 或新仓)"
    fi
  fi

  # RELEASE.md 覆盖率
  local has_release=0
  for path in RELEASE.md rust/RELEASE.md CHANGELOG.md; do
    if [ -f "$path" ]; then has_release=1; printf "  RELEASE doc: %s (%d 行)\n" "$path" "$(wc -l < "$path")"; fi
  done
  if [ "$has_release" -eq 0 ]; then err "$label: 无 RELEASE.md / CHANGELOG.md(违反 VERSIONING §3)"; fi

  # 最新 tag 是否在 RELEASE.md 出现
  if [ "$total" -gt 0 ] && [ "$has_release" -eq 1 ]; then
    local latest
    latest=$(git tag --sort=-creatordate | head -1)
    local found=0
    for path in RELEASE.md rust/RELEASE.md CHANGELOG.md; do
      [ -f "$path" ] || continue
      if grep -q "$latest\|${latest#v}" "$path" 2>/dev/null; then found=1; break; fi
    done
    if [ "$found" -eq 1 ]; then ok "最新 tag ($latest) 已写入 RELEASE doc"
    else warn "$label: 最新 tag ($latest) 未在任何 RELEASE doc 中找到"; fi
  fi
}

# ── attune × attune-pro 配对检查 ─────────────────────────────────────────────

check_pair_attune_pro() {
  echo
  bold "════════ attune ↔ attune-pro 配对检查 ════════"; echo

  cd "$ATTUNE" || { err "attune 仓不存在"; return; }
  local attune_latest_v
  # 取 v<X.Y.Z> 形态(排除 desktop-* / -rc.* / -beta.* 预发)
  attune_latest_v=$(git tag --sort=-creatordate | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1)

  cd "$ATTUNE_PRO" || { err "attune-pro 仓不存在"; return; }
  local pro_latest_v
  pro_latest_v=$(git tag --sort=-creatordate | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1)

  if [ -z "$attune_latest_v" ]; then warn "attune 无 vX.Y.Z 正式 tag"; return; fi
  if [ -z "$pro_latest_v" ]; then err "attune-pro 缺失对应 $attune_latest_v tag(违反 VERSIONING §5.1 配对)"; return; fi

  if [ "$attune_latest_v" = "$pro_latest_v" ]; then
    ok "配对一致:attune=$attune_latest_v, attune-pro=$pro_latest_v"
  else
    err "配对漂移:attune=$attune_latest_v, attune-pro=$pro_latest_v(应同号,违反 §5.1)"
  fi
}

# ── 主流程 ────────────────────────────────────────────────────────────────────

bold "Attune Ecosystem Version Audit — $(date '+%Y-%m-%d %H:%M:%S')"
echo

audit_repo "attune"           "$ATTUNE"     "main"
audit_repo "attune-pro"       "$ATTUNE_PRO" "main"
audit_repo "attune-pluginhub" "$PLUGINHUB"  "master"
audit_repo "cloud"            "$CLOUD"      "master"

check_pair_attune_pro

echo
bold "════════ 总结 ════════"; echo
if [ "$EXIT" -eq 0 ] && [ "$WARN" -eq 0 ]; then green "✅ 全部健康"; echo
elif [ "$EXIT" -eq 0 ]; then yellow "⚠️  有警告(详见上方)"; echo; EXIT=1
else red "❌ 有错误(详见上方)"; echo
fi
exit "$EXIT"
