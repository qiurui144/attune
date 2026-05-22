#!/usr/bin/env bash
# ga-ceremony.sh — v1.0.0 GA 三仓 ceremony 自动化
#
# 用法:
#   bash scripts/ga-ceremony.sh --dry-run     # 列出将要执行的操作，不执行
#   bash scripts/ga-ceremony.sh --execute     # 真实执行（交互确认后）
#
# 流程:
#   1. 预检（CI 绿 / working tree clean / develop push / 6 类下限 gate）
#   2. attune 仓：develop → main --no-ff + tag v1.0.0 + desktop-v1.0.0
#   3. attune-pro 仓：develop → main --no-ff + tag v1.0.0
#   4. cloud 仓：tag cloud-v2.2.0
#   5. push 三仓 + 所有 tag
#   6. 等待 GH Actions 触发，报告 release 链接
#
# 退出码: 0 成功 / 1 预检失败或用户中止 / 2 执行出错
#
# 依赖: git, gh (GitHub CLI), cargo (可选，6 类下限 gate)

set -uo pipefail

# ── 仓库路径（可通过环境变量覆盖） ────────────────────────────────────────────

ATTUNE="${ATTUNE_REPO:-/data/company/project/attune}"
ATTUNE_PRO="${ATTUNE_PRO_REPO:-/data/company/project/attune-pro}"
CLOUD="${CLOUD_REPO:-/data/company/cloud}"

# GA 版本常量
ATTUNE_VERSION="1.0.0"
CLOUD_TAG="cloud-v2.2.0"
ATTUNE_PRO_VERSION="1.0.0"

# ── 颜色辅助 ─────────────────────────────────────────────────────────────────

red()    { printf "\033[31m%s\033[0m\n" "$*"; }
green()  { printf "\033[32m%s\033[0m\n" "$*"; }
yellow() { printf "\033[33m%s\033[0m\n" "$*"; }
bold()   { printf "\033[1m%s\033[0m\n" "$*"; }
info()   { printf "  \033[36m→\033[0m %s\n" "$*"; }

ERRORS=0
err()  { red   "  [ERR]  $*"; ERRORS=$((ERRORS + 1)); }
warn() { yellow "  [WARN] $*"; }
ok()   { green  "  [OK]   $*"; }

# ── 模式解析 ─────────────────────────────────────────────────────────────────

MODE=""
for arg in "$@"; do
  case "$arg" in
    --dry-run)  MODE="dry-run" ;;
    --execute)  MODE="execute" ;;
    *)          echo "未知参数: $arg"; echo "用法: $0 --dry-run | --execute"; exit 1 ;;
  esac
done

if [ -z "$MODE" ]; then
  echo "用法: $0 --dry-run | --execute"
  exit 1
fi

# ── dry-run 执行器 ────────────────────────────────────────────────────────────
# dry_or_run <description> <cmd> [args...]
# dry-run 模式只打印；execute 模式真实执行

dry_or_run() {
  local desc="$1"; shift
  if [ "$MODE" = "dry-run" ]; then
    info "[DRY-RUN] $desc"
    info "          cmd: $*"
  else
    info "$desc"
    "$@" || { err "命令失败: $*"; return 1; }
  fi
}

# ── 标题 ─────────────────────────────────────────────────────────────────────

echo
bold "╔══════════════════════════════════════════════════════════════╗"
bold "║   attune v1.0.0 GA ceremony — $(date '+%Y-%m-%d %H:%M:%S')        ║"
bold "╚══════════════════════════════════════════════════════════════╝"
echo
bold "模式: $MODE"
echo
bold "版本配对:"
info "attune         v${ATTUNE_VERSION} + desktop-v${ATTUNE_VERSION}"
info "attune-pro     v${ATTUNE_PRO_VERSION}"
info "cloud          ${CLOUD_TAG}"
echo

# ═══════════════════════════════════════════════════════════════════════════
# STEP 1: 预检
# ═══════════════════════════════════════════════════════════════════════════

bold "══ STEP 1/5: 预检 ══════════════════════════════════════════════"
echo

# 1a. 三仓 working tree clean
check_clean() {
  local label="$1" dir="$2"
  if [ ! -d "$dir/.git" ]; then
    err "$label: 路径 $dir 不是 git 仓"; return
  fi
  local dirty
  # 只检查已跟踪文件的改动（忽略 untracked），避免 .claude/ / secrets/*.enc 等误触发
  dirty=$(git -C "$dir" status --porcelain 2>/dev/null | grep -v '^??')
  if [ -n "$dirty" ]; then
    err "$label working tree 不干净 — 请先 commit / stash"
    git -C "$dir" status --short | grep -v '^??' | head -10
  else
    ok "$label working tree clean"
  fi
}

check_clean "attune"     "$ATTUNE"
check_clean "attune-pro" "$ATTUNE_PRO"
check_clean "cloud"      "$CLOUD"

# 1b. develop 分支存在并已 push 到 remote
check_develop_pushed() {
  local label="$1" dir="$2"
  if ! git -C "$dir" rev-parse --verify develop >/dev/null 2>&1; then
    # cloud 仓使用 master 而非 develop，不强制要求 develop 分支
    warn "$label: develop 分支不存在（若该仓使用其他主分支则可忽略）"; return
  fi
  local local_sha remote_sha
  local_sha=$(git -C "$dir" rev-parse develop)
  remote_sha=$(git -C "$dir" rev-parse "origin/develop" 2>/dev/null || echo "")
  if [ -z "$remote_sha" ]; then
    err "$label: origin/develop 不存在 — 请先 push develop"
  elif [ "$local_sha" = "$remote_sha" ]; then
    ok "$label develop 已 push ($(echo "$local_sha" | cut -c1-8))"
  else
    err "$label: develop 与 origin/develop 不一致 — 请先 push"
    info "local:  $(echo "$local_sha"  | cut -c1-8)"
    info "remote: $(echo "$remote_sha" | cut -c1-8)"
  fi
}

check_develop_pushed "attune"     "$ATTUNE"
check_develop_pushed "attune-pro" "$ATTUNE_PRO"
check_develop_pushed "cloud"      "$CLOUD"

# 1c. attune CI 状态（develop 分支最近一次 workflow run）
check_ci() {
  local label="$1" repo_slug="$2" branch="$3"
  if ! command -v gh >/dev/null 2>&1; then
    warn "gh CLI 未安装，跳过 $label CI 检查"
    return
  fi
  local status conclusion
  status=$(gh run list --repo "$repo_slug" --branch "$branch" --limit 1 --json status,conclusion,displayTitle 2>/dev/null | \
    python3 -c "import sys,json; d=json.load(sys.stdin); r=d[0] if d else {}; print(r.get('status','?'), r.get('conclusion','?'), r.get('displayTitle','?')[:60])" 2>/dev/null || echo "? ? unknown")
  local st co title
  st=$(echo "$status" | awk '{print $1}')
  co=$(echo "$status" | awk '{print $2}')
  title=$(echo "$status" | cut -d' ' -f3-)
  if [ "$st" = "completed" ] && [ "$co" = "success" ]; then
    ok "$label CI 绿 ($branch) — $title"
  else
    warn "$label CI 状态: status=$st conclusion=$co — $title"
    warn "    → 建议确认 CI 已通过再执行 --execute"
  fi
}

# 从 remote URL 中提取 owner/repo
extract_slug() {
  git -C "$1" remote get-url origin 2>/dev/null | \
    sed -E 's,.*github\.com[:/],,; s,\.git$,,'
}

ATTUNE_SLUG=$(extract_slug "$ATTUNE")
ATTUNE_PRO_SLUG=$(extract_slug "$ATTUNE_PRO")

if [ -n "$ATTUNE_SLUG" ]; then
  check_ci "attune"     "$ATTUNE_SLUG"     "develop"
else
  warn "attune: 无法解析 GitHub slug，跳过 CI 检查"
fi
if [ -n "$ATTUNE_PRO_SLUG" ]; then
  check_ci "attune-pro" "$ATTUNE_PRO_SLUG" "develop"
else
  warn "attune-pro: 无法解析 GitHub slug，跳过 CI 检查"
fi

# 1d. attune-pro 6 类下限 gate（可选，需 ATTUNE_ENFORCE_SIX_CATEGORY_FLOOR=1）
echo
info "检查 6 类下限 gate..."
if [ "${ATTUNE_ENFORCE_SIX_CATEGORY_FLOOR:-0}" = "1" ]; then
  if [ "$MODE" = "dry-run" ]; then
    info "[DRY-RUN] ATTUNE_ENFORCE_SIX_CATEGORY_FLOOR=1 cargo test -p law-pro six_category_floor (in attune-pro)"
  else
    info "运行 attune-pro 6 类下限 gate..."
    if (cd "$ATTUNE_PRO" && ATTUNE_ENFORCE_SIX_CATEGORY_FLOOR=1 cargo test -p law-pro six_category_floor 2>&1); then
      ok "6 类下限 gate PASS"
    else
      err "6 类下限 gate FAIL — 必须修复后再执行 GA"
    fi
  fi
else
  warn "ATTUNE_ENFORCE_SIX_CATEGORY_FLOOR 未设为 1，跳过 6 类下限 gate"
  warn "  建议: export ATTUNE_ENFORCE_SIX_CATEGORY_FLOOR=1 后重跑"
fi

# 1e. RELEASE.md v1.0.0 节存在
check_release_section() {
  local label="$1" file="$2" version="$3"
  if [ ! -f "$file" ]; then
    err "$label: RELEASE doc $file 不存在"; return
  fi
  if grep -qE "^## (v)?${version}" "$file" 2>/dev/null; then
    ok "$label RELEASE.md 有 v${version} 节"
  else
    err "$label: RELEASE.md 缺 v${version} 节 — 请补充后再 GA"
  fi
}

check_release_section "attune"     "$ATTUNE/rust/RELEASE.md"     "$ATTUNE_VERSION"
check_release_section "attune-pro" "$ATTUNE_PRO/RELEASE.md"      "$ATTUNE_PRO_VERSION"
check_release_section "cloud"      "$CLOUD/RELEASE.md"           "2.2.0"

# 1f. 版本字段一致性（Cargo.toml / plugin.yaml / tauri.conf.json）
check_cargo_version() {
  local label="$1" cargo_toml="$2" expected="$3"
  if [ ! -f "$cargo_toml" ]; then warn "$label: $cargo_toml 不存在，跳过"; return; fi
  local ver
  ver=$(grep -E '^version\s*=' "$cargo_toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
  if [ "$ver" = "$expected" ]; then
    ok "$label Cargo.toml workspace version = $ver"
  else
    err "$label Cargo.toml workspace version=$ver，期望 $expected"
  fi
}

# attune-pro workspace 无顶层 version=，使用 law-pro crate 版本代表整包
check_cargo_version_crate() {
  local label="$1" cargo_toml="$2" expected="$3"
  if [ ! -f "$cargo_toml" ]; then warn "$label: $cargo_toml 不存在，跳过"; return; fi
  local ver
  ver=$(grep -E '^version\s*=' "$cargo_toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
  if [ "$ver" = "$expected" ]; then
    ok "$label Cargo.toml (law-pro crate) version = $ver"
  else
    err "$label Cargo.toml (law-pro crate) version=$ver，期望 $expected"
  fi
}

check_cargo_version "attune" "$ATTUNE/rust/Cargo.toml" "$ATTUNE_VERSION"
# attune-pro: workspace 无顶层 version；检查 law-pro crate（代表 plugin pack 版本）
check_cargo_version_crate "attune-pro" "$ATTUNE_PRO/plugins/law-pro/Cargo.toml" "$ATTUNE_PRO_VERSION"

# plugin.yaml version + attune_min_version
PLUGIN_YAML="$ATTUNE_PRO/plugins/law-pro/plugin.yaml"
if [ -f "$PLUGIN_YAML" ]; then
  py_ver=$(grep -E '^version:' "$PLUGIN_YAML" | head -1 | sed 's/version: *"\?\([^"]*\)"\?/\1/')
  py_min=$(grep -E '^attune_min_version:' "$PLUGIN_YAML" | head -1 | sed 's/attune_min_version: *"\?\([^"]*\)"\?/\1/')
  if [ "$py_ver" = "$ATTUNE_PRO_VERSION" ]; then
    ok "law-pro plugin.yaml version = $py_ver"
  else
    err "law-pro plugin.yaml version=$py_ver，期望 $ATTUNE_PRO_VERSION"
  fi
  if [ "$py_min" = "$ATTUNE_VERSION" ]; then
    ok "law-pro plugin.yaml attune_min_version = $py_min"
  else
    err "law-pro plugin.yaml attune_min_version=$py_min，期望 $ATTUNE_VERSION"
  fi
else
  err "law-pro plugin.yaml 不存在: $PLUGIN_YAML"
fi

# tauri.conf.json version
TAURI_CONF="$ATTUNE/apps/attune-desktop/tauri.conf.json"
if [ -f "$TAURI_CONF" ]; then
  tauri_ver=$(python3 -c "import json; d=json.load(open('$TAURI_CONF')); print(d.get('version','') or d.get('package',{}).get('version',''))" 2>/dev/null || echo "")
  if [ "$tauri_ver" = "$ATTUNE_VERSION" ]; then
    ok "tauri.conf.json version = $tauri_ver"
  else
    err "tauri.conf.json version=$tauri_ver，期望 $ATTUNE_VERSION"
  fi
else
  warn "tauri.conf.json 不存在: $TAURI_CONF，跳过"
fi

# cloud RELEASE.md cloud-v2.2.0 节
if grep -qE "^## ${CLOUD_TAG}" "$CLOUD/RELEASE.md" 2>/dev/null || \
   grep -qE "^## v2\.2\.0"     "$CLOUD/RELEASE.md" 2>/dev/null; then
  ok "cloud RELEASE.md 有 ${CLOUD_TAG} / v2.2.0 节"
else
  err "cloud RELEASE.md 缺 ${CLOUD_TAG} 节"
fi

echo

if [ "$ERRORS" -gt 0 ]; then
  red "预检失败：$ERRORS 个错误，请修复后重跑"
  echo
  exit 1
fi

ok "预检全部通过 ✅"
echo

# ═══════════════════════════════════════════════════════════════════════════
# dry-run 输出完整计划后退出
# ═══════════════════════════════════════════════════════════════════════════

if [ "$MODE" = "dry-run" ]; then
  bold "══ DRY-RUN 完整操作计划 ════════════════════════════════════════"
  echo
  bold "STEP 2: attune 仓"
  info "git -C $ATTUNE checkout main && git -C $ATTUNE pull origin main"
  info "git -C $ATTUNE merge --no-ff develop -m 'merge: develop → main (v1.0.0 GA)'"
  info "git -C $ATTUNE tag -a v${ATTUNE_VERSION}         -m '... (从 rust/RELEASE.md 生成)'"
  info "git -C $ATTUNE tag -a desktop-v${ATTUNE_VERSION} -m '... (从 rust/RELEASE.md 生成)'"
  info "git -C $ATTUNE push origin main v${ATTUNE_VERSION} desktop-v${ATTUNE_VERSION}"
  echo
  bold "STEP 3: attune-pro 仓"
  info "git -C $ATTUNE_PRO checkout main && git -C $ATTUNE_PRO pull origin main"
  info "git -C $ATTUNE_PRO merge --no-ff develop -m 'merge: develop → main (v1.0.0 GA)'"
  info "git -C $ATTUNE_PRO tag -a v${ATTUNE_PRO_VERSION} -m '... (从 RELEASE.md 生成)'"
  info "git -C $ATTUNE_PRO push origin main v${ATTUNE_PRO_VERSION}"
  echo
  bold "STEP 4: cloud 仓"
  info "git -C $CLOUD tag -a ${CLOUD_TAG} -m '... (从 RELEASE.md 生成)'"
  info "git -C $CLOUD push origin ${CLOUD_TAG}"
  echo
  bold "STEP 5: 等待 GH Actions"
  info "gh run list --repo $ATTUNE_SLUG --limit 5 (触发 rust-release.yml + desktop-release.yml)"
  info "预计产物: 5 平台 server/CLI tarball + NSIS / MSI / .deb / RPM / AppImage"
  echo
  green "DRY-RUN 完成 — 使用 --execute 真实执行"
  echo
  exit 0
fi

# ═══════════════════════════════════════════════════════════════════════════
# execute 模式：用户确认
# ═══════════════════════════════════════════════════════════════════════════

bold "══ EXECUTE 确认 ═════════════════════════════════════════════════"
echo
yellow "即将执行 v1.0.0 GA ceremony，操作不可撤销（tag 一旦 push）。"
echo
printf "请确认三仓 develop 状态已最终 review。输入 'yes' 继续，其他任意键中止: "
read -r answer
if [ "$answer" != "yes" ]; then
  yellow "用户中止，未执行任何操作。"
  exit 0
fi
echo

# ── 从 RELEASE.md 提取 v1.0.0 tag 消息 ───────────────────────────────────

extract_tag_msg() {
  local release_md="$1" version="$2"
  # 取 ## v<version> 到下一个 ## 之间的前 10 行作为 tag msg
  awk "/^## (v)?${version}/{found=1; next} found && /^## /{exit} found{print}" "$release_md" 2>/dev/null | \
    grep -v '^[[:space:]]*$' | head -10 | sed 's/^[[:space:]]*//'
}

ATTUNE_TAG_BODY=$(extract_tag_msg "$ATTUNE/rust/RELEASE.md" "$ATTUNE_VERSION")
PRO_TAG_BODY=$(extract_tag_msg "$ATTUNE_PRO/RELEASE.md"     "$ATTUNE_PRO_VERSION")
CLOUD_TAG_BODY=$(extract_tag_msg "$CLOUD/RELEASE.md"         "2.2.0")

# ═══════════════════════════════════════════════════════════════════════════
# STEP 2: attune 仓
# ═══════════════════════════════════════════════════════════════════════════

bold "══ STEP 2/5: attune 仓 ceremony ═════════════════════════════════"
echo

(
  cd "$ATTUNE" || { err "无法 cd $ATTUNE"; exit 1; }

  dry_or_run "切换到 main 并 pull" git checkout main
  dry_or_run "拉取 origin/main"    git pull origin main

  dry_or_run "merge develop → main (--no-ff)" \
    git merge --no-ff develop -m "merge: develop → main (v${ATTUNE_VERSION} GA)"

  dry_or_run "打 annotated tag v${ATTUNE_VERSION}" \
    git tag -a "v${ATTUNE_VERSION}" -m "v${ATTUNE_VERSION} GA: 私有 AI 知识伙伴首个 1.x 正式版

${ATTUNE_TAG_BODY}

配对: attune-pro v${ATTUNE_PRO_VERSION} + ${CLOUD_TAG}"

  dry_or_run "打 annotated tag desktop-v${ATTUNE_VERSION}" \
    git tag -a "desktop-v${ATTUNE_VERSION}" -m "desktop-v${ATTUNE_VERSION} GA: Tauri 桌面安装器

产物: NSIS / MSI / .deb / RPM / AppImage（5 平台）
配对: attune v${ATTUNE_VERSION}"

  dry_or_run "push main + 两个 tag" \
    git push origin main "v${ATTUNE_VERSION}" "desktop-v${ATTUNE_VERSION}"

  ok "attune 仓 ceremony 完成"
)
[ "$ERRORS" -eq 0 ] || { red "attune 仓 ceremony 失败，中止"; exit 2; }

# ═══════════════════════════════════════════════════════════════════════════
# STEP 3: attune-pro 仓
# ═══════════════════════════════════════════════════════════════════════════

bold "══ STEP 3/5: attune-pro 仓 ceremony ════════════════════════════"
echo

(
  cd "$ATTUNE_PRO" || { err "无法 cd $ATTUNE_PRO"; exit 1; }

  dry_or_run "切换到 main 并 pull" git checkout main
  dry_or_run "拉取 origin/main"    git pull origin main

  dry_or_run "merge develop → main (--no-ff)" \
    git merge --no-ff develop -m "merge: develop → main (v${ATTUNE_PRO_VERSION} GA)"

  dry_or_run "打 annotated tag v${ATTUNE_PRO_VERSION}" \
    git tag -a "v${ATTUNE_PRO_VERSION}" -m "v${ATTUNE_PRO_VERSION} GA: law-pro plugin pack 首个正式版

${PRO_TAG_BODY}

配对: attune v${ATTUNE_VERSION} + ${CLOUD_TAG}"

  dry_or_run "push main + tag" \
    git push origin main "v${ATTUNE_PRO_VERSION}"

  ok "attune-pro 仓 ceremony 完成"
)
[ "$ERRORS" -eq 0 ] || { red "attune-pro 仓 ceremony 失败，中止"; exit 2; }

# ═══════════════════════════════════════════════════════════════════════════
# STEP 4: cloud 仓
# ═══════════════════════════════════════════════════════════════════════════

bold "══ STEP 4/5: cloud 仓 ceremony ══════════════════════════════════"
echo

(
  cd "$CLOUD" || { err "无法 cd $CLOUD"; exit 1; }

  # cloud 只打 tag，不做 develop → main merge（cloud 有自己的分支管理）
  dry_or_run "打 annotated tag ${CLOUD_TAG}" \
    git tag -a "${CLOUD_TAG}" -m "${CLOUD_TAG}: attune v1.0 GA 配套发版

${CLOUD_TAG_BODY}

兼容矩阵: 支持 attune client v1.0.x"

  CLOUD_SLUG=$(git remote get-url origin 2>/dev/null | sed -E 's,.*github\.com[:/],,; s,\.git$,,')
  if [ -n "$CLOUD_SLUG" ]; then
    dry_or_run "push cloud tag" git push origin "${CLOUD_TAG}"
  else
    warn "cloud remote 不是 github.com，请手动 push ${CLOUD_TAG}"
  fi

  ok "cloud 仓 ceremony 完成"
)

# ═══════════════════════════════════════════════════════════════════════════
# STEP 5: 等待 GH Actions + 报告
# ═══════════════════════════════════════════════════════════════════════════

bold "══ STEP 5/5: GH Actions 监控 ════════════════════════════════════"
echo

if command -v gh >/dev/null 2>&1 && [ -n "$ATTUNE_SLUG" ]; then
  info "等待 5 秒让 GH Actions 触发..."
  sleep 5

  info "rust-release.yml 最新 run:"
  gh run list --repo "$ATTUNE_SLUG" --workflow rust-release.yml    --limit 3 2>/dev/null || true
  echo
  info "desktop-release.yml 最新 run:"
  gh run list --repo "$ATTUNE_SLUG" --workflow desktop-release.yml --limit 3 2>/dev/null || true
  echo
  info "GH Releases 页面: https://github.com/${ATTUNE_SLUG}/releases"
  info "需确认以下产物出现:"
  info "  server/CLI: linux-x86_64 / linux-aarch64 / windows-x86_64 / macos-apple-silicon"
  info "  桌面:       NSIS (.exe) / MSI / .deb / .rpm / AppImage"
else
  warn "gh CLI 不可用或 slug 未解析，请手动检查:"
  info "  https://github.com/<owner>/attune/actions"
fi

echo
bold "══════════════════════════════════════════════════════════════════"
green "v1.0.0 GA ceremony 完成！"
echo
info "后续核查清单:"
info "  [ ] GH Releases 有 v1.0.0 + desktop-v1.0.0 双 release page"
info "  [ ] 5 平台产物全出（tarball + 安装包）"
info "  [ ] attune-pro v1.0.0 tag 在 GitHub 可见"
info "  [ ] cloud ${CLOUD_TAG} tag push 成功"
info "  [ ] 三仓 main 都已更新到 GA commit"
echo

exit 0
