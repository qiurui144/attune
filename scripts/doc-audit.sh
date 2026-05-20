#!/usr/bin/env bash
# doc-audit.sh — 文档体系审计(per 全局 CLAUDE.md 文档体系铁律)
#
# 验证当前仓的 .md 是否符合白名单 + 检出禁止形态。
#
# 退出码:0 健康 / 1 有警告 / 2 有错误
# 见 全局 CLAUDE.md「文档体系铁律」节

set -uo pipefail

REPO="${1:-$(pwd)}"
cd "$REPO" || { echo "无法 cd $REPO"; exit 2; }

# 排除路径正则:test 语料 / 外部下载内容 / vendored 依赖 / build 产物
# 这些目录的 .md 是外部 upstream 内容,不是我们的项目文档
EXCLUDE_RE='(^|/)(node_modules|target|\.git|tests/corpora|tests/snapshots|vendor|third_party|\.next|build|dist)(/|$)'

EXIT=0
WARN=0
red()   { printf "\033[31m%s\033[0m" "$*"; }
green() { printf "\033[32m%s\033[0m" "$*"; }
yellow(){ printf "\033[33m%s\033[0m" "$*"; }
bold()  { printf "\033[1m%s\033[0m" "$*"; }

err()  { red    "ERR " ; echo " $*"; EXIT=2; }
warn() { yellow "WARN" ; echo " $*"; WARN=1; }
ok()   { green  "OK  " ; echo " $*"; }

bold "Doc audit — $REPO"; echo

# ── 1. 根目录 .md 白名单 ─────────────────────────────────────────────────────
ROOT_ALLOW=(README.md README.zh.md DEVELOP.md RELEASE.md CLAUDE.md ACKNOWLEDGMENTS.md ACKNOWLEDGMENTS.zh.md LICENSE LICENSE.md AGENTS.md GEMINI.md)

bold "─ 根目录 .md 白名单 ─"; echo
violations=()
while IFS= read -r f; do
  base=$(basename "$f")
  hit=0
  for allowed in "${ROOT_ALLOW[@]}"; do [ "$base" = "$allowed" ] && { hit=1; break; }; done
  [ "$hit" -eq 0 ] && violations+=("$f")
done < <(find . -maxdepth 1 -name "*.md" -type f 2>/dev/null)

if [ "${#violations[@]}" -eq 0 ]; then
  ok "根目录 .md 全部在白名单"
else
  err "根目录有 ${#violations[@]} 个不在白名单的 .md(应归并 / 删除):"
  for v in "${violations[@]}"; do echo "    $v"; done
fi
echo

# ── 2. 禁止形态扫描 ─────────────────────────────────────────────────────────
bold "─ 禁止形态扫描 ─"; echo

# 2a release notes 独立文件
banned=$(find . \( -name "v*-release*.md" -o -name "v*-rc*-test*.md" \) 2>/dev/null | grep -vE "$EXCLUDE_RE" | sort)
if [ -n "$banned" ]; then
  err "release notes 独立文件(应入 RELEASE.md):"
  echo "$banned" | sed 's/^/    /'
else
  ok "无 vX-release-notes.md 残留"
fi
echo

# 2b tasks / report / analysis / readiness
banned=$(find . \( -name "*-tasks.md" -o -name "*-todo.md" -o -name "*-report.md" -o -name "*-analysis.md" -o -name "*-readiness.md" \) 2>/dev/null | grep -vE "$EXCLUDE_RE" | grep -v "/superpowers/plans/" | sort)
if [ -n "$banned" ]; then
  warn "一次性文档(应入 PR / RELEASE / ADR,不留独立 .md):"
  echo "$banned" | sed 's/^/    /'
else
  ok "无 *-tasks/report/analysis/readiness .md 残留"
fi
echo

# 2c 非 README 双语副本
banned=$(find . -name "*.zh.md" -not -name "README.zh.md" -not -name "ACKNOWLEDGMENTS.zh.md" 2>/dev/null | grep -vE "$EXCLUDE_RE" | sort)
if [ -n "$banned" ]; then
  err "技术 / 开发文档不应双语(只 README 允许 .zh.md):"
  echo "$banned" | sed 's/^/    /' | head -20
  total=$(echo "$banned" | wc -l)
  [ "$total" -gt 20 ] && echo "    ... 等 $total 个"
else
  ok "无非 README 双语副本"
fi
echo

# 2d USER-* 文档
banned=$(find . \( -name "USER-*.md" -o -name "*-USER.md" \) 2>/dev/null | grep -vE "$EXCLUDE_RE" | sort)
if [ -n "$banned" ]; then
  err "USER-* 类用户视角文档应入 README:"
  echo "$banned" | sed 's/^/    /'
else
  ok "无 USER-* 残留"
fi
echo

# 2e CHANGELOG.md + RELEASE.md 并存
if [ -f CHANGELOG.md ] && [ -f RELEASE.md ]; then
  warn "CHANGELOG.md 与 RELEASE.md 并存(应二选一,推荐 RELEASE.md)"
fi
echo

# 2f screenshots 多目录
ss_dirs=$(find . -maxdepth 3 -type d -name "screenshots*" 2>/dev/null | grep -vE "$EXCLUDE_RE" | sort)
ss_count=$(echo "$ss_dirs" | grep -c . 2>/dev/null)
if [ "$ss_count" -gt 1 ]; then
  warn "多个 screenshots 目录(应合一):"
  echo "$ss_dirs" | sed 's/^/    /'
fi
echo

# ── 3. 同主题重复扫描(启发式 keyword 重叠) ─────────────────────────────────
bold "─ 同主题重复扫描(启发式) ─"; echo

for kw in FEATURES RELEASE CHANGELOG INSTALL TESTING DEPLOY; do
  hits=$(find . -name "*$kw*.md" 2>/dev/null | grep -vE "$EXCLUDE_RE" | sort)
  count=$(echo "$hits" | grep -c . 2>/dev/null)
  if [ "$count" -gt 1 ]; then
    warn "主题 '$kw' 命中 $count 份(可能重复):"
    echo "$hits" | head -5 | sed 's/^/    /'
  fi
done
echo

# ── 4. docs/ 总计 ────────────────────────────────────────────────────────────
bold "─ docs/ 规模 ─"; echo
md_count=$(find docs/ -name "*.md" -type f 2>/dev/null | grep -vE "$EXCLUDE_RE" | wc -l)
md_lines=$(find docs/ -name "*.md" -type f 2>/dev/null | grep -vE "$EXCLUDE_RE" | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1}')
echo "  *.md 文件数: $md_count  总行数: $md_lines"
[ "$md_count" -gt 30 ] && warn "docs/ 文件 > 30 份,建议巡视是否有可归并的"
echo

# ── 总结 ─────────────────────────────────────────────────────────────────────
bold "═══ 总结 ═══"; echo
if [ "$EXIT" -eq 0 ] && [ "$WARN" -eq 0 ]; then green "✅ 文档体系健康"; echo
elif [ "$EXIT" -eq 0 ]; then yellow "⚠️  有警告(详见上方,可考虑归并)"; echo; EXIT=1
else red "❌ 有错误(详见上方,违反铁律)"; echo
fi
exit "$EXIT"
