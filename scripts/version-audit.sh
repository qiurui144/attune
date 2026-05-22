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

# GA 专项检查模式：VERSIONING_GA_CHECK=1 时额外验证三仓版本字段全对齐 v1.0.0
GA_CHECK="${VERSIONING_GA_CHECK:-0}"

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
  # 注意:tag 形态可能含命名空间前缀(cloud-v2.1.0 / desktop-v0.7.0 / law-pro/v0.5.4)
  # 而 RELEASE.md 节标题通常写无前缀的 "## v2.1.0"。剥前缀后再 grep。
  if [ "$total" -gt 0 ] && [ "$has_release" -eq 1 ]; then
    local latest stripped
    latest=$(git tag --sort=-creatordate | head -1)
    # 剥剥前缀:cloud-v / desktop-v / <ns>/v / 单纯 v → 得到 "X.Y.Z"
    stripped=$(echo "$latest" | sed -E 's,^(cloud-|desktop-)?v,,; s,^[a-z_-]+/v,,')
    local found=0
    for path in RELEASE.md rust/RELEASE.md CHANGELOG.md; do
      [ -f "$path" ] || continue
      # 检查原 tag 字面量 OR 剥前缀后的 vX.Y.Z 形态
      if grep -qE "$latest|v$stripped\b|^## *v$stripped" "$path" 2>/dev/null; then found=1; break; fi
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
  # 取最新的 v<X.Y.Z>[-rc.N/-beta.N/-alpha.N] 形态(排除 desktop-*)
  # GA 前 rc 阶段两仓都在同号 rc，用含 rc 的 tag 对比才能正确配对
  attune_latest_v=$(git tag --sort=-creatordate | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+' | grep -v '^desktop-' | head -1)

  cd "$ATTUNE_PRO" || { err "attune-pro 仓不存在"; return; }
  local pro_latest_v
  pro_latest_v=$(git tag --sort=-creatordate | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+' | grep -v '^desktop-' | head -1)

  if [ -z "$attune_latest_v" ]; then warn "attune 无 vX.Y.Z 正式 tag"; return; fi
  if [ -z "$pro_latest_v" ]; then err "attune-pro 缺失对应 $attune_latest_v tag(违反 VERSIONING §5.1 配对)"; return; fi

  if [ "$attune_latest_v" = "$pro_latest_v" ]; then
    ok "配对一致:attune=$attune_latest_v, attune-pro=$pro_latest_v"
  else
    err "配对漂移:attune=$attune_latest_v, attune-pro=$pro_latest_v(应同号,违反 §5.1)"
  fi
}

# ── GA v1.0.0 专项版本字段配对检查 ───────────────────────────────────────────
# 验证: Cargo.toml workspace version / tauri.conf.json / plugin.yaml version +
#        attune_min_version / cloud RELEASE.md 节 / cloud-v2.2.0 tag
# 仅在 VERSIONING_GA_CHECK=1 时启用；或通过 GA_CHECK 变量控制

check_ga_version_fields() {
  [ "$GA_CHECK" = "1" ] || return 0

  local ga_ver="1.0.0"
  local cloud_tag="cloud-v2.2.0"
  local ga_errors=0

  echo
  bold "════════ GA v${ga_ver} 版本字段对齐审计 ════════"; echo

  # 1. attune Cargo.toml workspace version
  local acv
  acv=$(grep -E '^version\s*=' "$ATTUNE/rust/Cargo.toml" 2>/dev/null | head -1 | sed 's/.*"\(.*\)".*/\1/')
  if [ "$acv" = "$ga_ver" ]; then
    ok "attune rust/Cargo.toml workspace version = $acv"
  else
    err "attune rust/Cargo.toml workspace version=$acv，期望 $ga_ver"; ga_errors=$((ga_errors+1))
  fi

  # 2. tauri.conf.json version
  local tauri_conf="$ATTUNE/apps/attune-desktop/tauri.conf.json"
  if [ -f "$tauri_conf" ]; then
    local tv
    tv=$(python3 -c "import json; d=json.load(open('$tauri_conf')); print(d.get('version','') or d.get('package',{}).get('version',''))" 2>/dev/null || echo "")
    if [ "$tv" = "$ga_ver" ]; then
      ok "tauri.conf.json version = $tv"
    else
      err "tauri.conf.json version=$tv，期望 $ga_ver"; ga_errors=$((ga_errors+1))
    fi
  else
    warn "tauri.conf.json 不存在: $tauri_conf，跳过"
  fi

  # 3. attune-pro 各 plugin crate 版本（workspace 无顶层 version=，用 law-pro 代表）
  local pcv
  pcv=$(grep -E '^version\s*=' "$ATTUNE_PRO/plugins/law-pro/Cargo.toml" 2>/dev/null | head -1 | sed 's/.*"\(.*\)".*/\1/')
  if [ "$pcv" = "$ga_ver" ]; then
    ok "attune-pro law-pro crate Cargo.toml version = $pcv"
  else
    err "attune-pro law-pro crate Cargo.toml version=$pcv，期望 $ga_ver"; ga_errors=$((ga_errors+1))
  fi

  # 4. law-pro plugin.yaml version + attune_min_version
  local plugin_yaml="$ATTUNE_PRO/plugins/law-pro/plugin.yaml"
  if [ -f "$plugin_yaml" ]; then
    local pyv pym
    pyv=$(grep -E '^version:' "$plugin_yaml" | head -1 | sed 's/version: *"\?\([^"]*\)"\?.*/\1/')
    pym=$(grep -E '^attune_min_version:' "$plugin_yaml" | head -1 | sed 's/attune_min_version: *"\?\([^"]*\)"\?.*/\1/')
    if [ "$pyv" = "$ga_ver" ]; then
      ok "law-pro plugin.yaml version = $pyv"
    else
      err "law-pro plugin.yaml version=$pyv，期望 $ga_ver"; ga_errors=$((ga_errors+1))
    fi
    if [ "$pym" = "$ga_ver" ]; then
      ok "law-pro plugin.yaml attune_min_version = $pym"
    else
      err "law-pro plugin.yaml attune_min_version=$pym，期望 $ga_ver"; ga_errors=$((ga_errors+1))
    fi
  else
    err "law-pro plugin.yaml 不存在: $plugin_yaml"; ga_errors=$((ga_errors+1))
  fi

  # 5. cloud RELEASE.md 有 cloud-v2.2.0 节
  if [ -f "$CLOUD/RELEASE.md" ]; then
    if grep -qE "^## (${cloud_tag}|v2\.2\.0)" "$CLOUD/RELEASE.md" 2>/dev/null; then
      ok "cloud RELEASE.md 有 ${cloud_tag} / v2.2.0 节"
    else
      err "cloud RELEASE.md 缺 ${cloud_tag} 节"; ga_errors=$((ga_errors+1))
    fi
  else
    err "cloud RELEASE.md 不存在: $CLOUD/RELEASE.md"; ga_errors=$((ga_errors+1))
  fi

  # 6. cloud 仓 cloud-v2.2.0 tag 是否已存在（发版前应未创建；用作可选检查）
  if git -C "$CLOUD" rev-parse "$cloud_tag" >/dev/null 2>&1; then
    warn "cloud 仓 $cloud_tag tag 已存在（GA ceremony 已完成或需检查）"
  else
    ok "cloud 仓 $cloud_tag tag 尚未创建（ceremony 前正常）"
  fi

  echo
  if [ "$ga_errors" -eq 0 ]; then
    green "✅ GA v${ga_ver} 版本字段全部对齐"; echo
  else
    red "❌ GA 版本字段 $ga_errors 处不对齐，请修复"; echo
    EXIT=2
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
check_ga_version_fields

echo
bold "════════ 总结 ════════"; echo
if [ "$EXIT" -eq 0 ] && [ "$WARN" -eq 0 ]; then green "✅ 全部健康"; echo
elif [ "$EXIT" -eq 0 ]; then yellow "⚠️  有警告(详见上方)"; echo; EXIT=1
else red "❌ 有错误(详见上方)"; echo
fi
exit "$EXIT"
