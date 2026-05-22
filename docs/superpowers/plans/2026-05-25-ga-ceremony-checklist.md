# v1.0.0 GA Ceremony — 5/25 当天执行清单

**使用方式**：5/25 GA 日按顺序逐项打勾。所有项目通过后执行 `--execute`。
执行完成后本文件可删除（ceremony 结论记入 RELEASE.md v1.0.0 节）。

---

## Phase A — 执行前确认（GA-1 天，5/24 完成）

- [ ] `git log --oneline origin/develop..develop` = 空（develop 已全部 push）
- [ ] attune develop CI 最近一次 run 全绿
- [ ] attune-pro develop CI 最近一次 run 全绿
- [ ] `VERSIONING_GA_CHECK=1 bash scripts/version-audit.sh` 全部 OK（无 ERR / 无 WARN）
- [ ] RELEASE.md v1.0.0 节内容已 final review（Highlights / Known Limitations / Breaking changes）
- [ ] attune-pro RELEASE.md v1.0.0 节内容已 final review
- [ ] cloud RELEASE.md v2.2.0 节内容已 final review
- [ ] `bash scripts/ga-ceremony.sh --dry-run` 预演无报错，输出符合预期

---

## Phase B — GA 当天执行

### B1. 版本字段最终核查

- [ ] `grep '^version' rust/Cargo.toml` → `"1.0.0"`
- [ ] `python3 -c "import json; print(json.load(open('apps/attune-desktop/tauri.conf.json')).get('version'))"` → `1.0.0`
- [ ] attune-pro `grep '^version' Cargo.toml` → `"1.0.0"`（注意：attune-pro workspace 中个别 crate 版本需一致）
- [ ] attune-pro `plugins/law-pro/plugin.yaml` → `version: "1.0.0"` + `attune_min_version: "1.0.0"`
- [ ] cloud `RELEASE.md` 有 `## cloud-v2.2.0` 节

### B2. 6 类下限 gate

- [ ] `export ATTUNE_ENFORCE_SIX_CATEGORY_FLOOR=1 && cd attune-pro && cargo test -p law-pro six_category_floor 2>&1 | tail -5` → 全 PASS

### B3. 执行 ceremony

- [ ] 用户拍板：口头确认可以执行
- [ ] `bash scripts/ga-ceremony.sh --execute` → 在三仓操作完成后输出 "v1.0.0 GA ceremony 完成！"

---

## Phase C — 发版后验证

### C1. Tag 确认

- [ ] `git -C attune tag | grep v1.0.0` 输出 `v1.0.0` 和 `desktop-v1.0.0`
- [ ] `git -C attune log --oneline origin/main | head -1` → merge commit 含 "v1.0.0 GA"
- [ ] `git -C attune-pro tag | grep v1.0.0` 输出 `v1.0.0`
- [ ] `git -C cloud tag | grep cloud-v2.2.0` 输出 `cloud-v2.2.0`

### C2. GitHub Releases 页面

- [ ] attune GitHub Releases 有 `v1.0.0` release page（rust-release.yml 产物）
- [ ] attune GitHub Releases 有 `desktop-v1.0.0` release page（desktop-release.yml 产物）

### C3. Release 产物完整性

server/CLI tarball（`rust-release.yml`）：
- [ ] `attune-server-v1.0.0-linux-x86_64.tar.gz` + SHA256
- [ ] `attune-server-v1.0.0-linux-aarch64.tar.gz` + SHA256
- [ ] `attune-server-v1.0.0-windows-x86_64.zip` + SHA256
- [ ] `attune-server-v1.0.0-macos-aarch64.tar.gz` + SHA256

桌面安装包（`desktop-release.yml`）：
- [ ] Windows NSIS installer (`.exe`)
- [ ] Windows MSI (`.msi`)
- [ ] Linux `.deb` (x86_64)
- [ ] Linux `.rpm` (x86_64)
- [ ] Linux AppImage

### C4. 三仓 main 状态

- [ ] `git -C attune log origin/main --first-parent --oneline | head -3` — 最新是 merge commit，无裸 commit
- [ ] `git -C attune-pro log origin/main --first-parent --oneline | head -3` — 同上
- [ ] `git -C cloud log origin/master --oneline | head -1` — cloud-v2.2.0 tag 在正确 commit

### C5. 上架确认（5/26）

- [ ] cloud SaaS accounts 服务 healthcheck 绿（`/health` → 200）
- [ ] cloud pluginhub 服务 healthcheck 绿
- [ ] cloud official-web 可访问
- [ ] cloud wiki-web 可访问
- [ ] attune-pro v1.0.0 可通过 pluginhub 正常下载（trial quota 生效）

---

## 已知遗留缺口（不阻塞 GA，记入 v1.0.1）

- `law-pro::defamation` 真 LLM F1=0.56（目标 ≥0.75）→ v1.0.1 prompt 强化
- 弱模型矩阵 #68（gemma:2b / phi3:mini holdout）→ v1.0.1
- Linux ARM64 桌面 .deb 不在 desktop-v1.0.0 产物（server CLI tar 已覆盖）→ desktop-v1.0.1

---

*本文件是一次性运行手册。ceremony 完成后删除，结论汇入 RELEASE.md v1.0.0 节。*
