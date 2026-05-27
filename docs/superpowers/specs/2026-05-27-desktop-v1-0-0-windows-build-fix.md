# desktop-v1.0.0 Windows build fix

- **Date**: 2026-05-27
- **Run failed**: https://github.com/qiurui144/attune/actions/runs/26385089128 (job 77661823857)
- **Tag affected**: `desktop-v1.0.0` (GA, 2026-05-25) — release page 仅含 3 Linux artifact (.deb / .rpm / .AppImage), Windows .exe / .msi 缺失
- **Severity**: P0 — Windows 用户无法获取 v1.0.0 installer (~50% target 用户群)

## 1. 目标定位

修复 `desktop-release.yml` Windows runner build fail, 让后续 desktop-v1.0.0 / v1.0.1 build 同时产 Linux + Windows artifact, 与 README "Windows / Linux 双轨" 定位对齐.

## 2. 范围边界

**做**:
- 修 `.github/workflows/desktop-release.yml` "Build Tauri bundles" 步骤 shell 声明
- 落档本 spec 记录 root cause + fix + re-trigger 路径
- workflow_dispatch 重 build `desktop-v1.0.0` 让 Windows artifact 补到现有 release page

**不做**:
- 不动 v1.0.0 GA tag (per CLAUDE.md "tag 一旦 push 视为不可撤销")
- 不改 Tauri / Rust 代码逻辑 (纯 CI infra fix)
- 不预先做 v1.0.1 prep (有独立 spec `2026-05-26-v1-0-1-upgrade-strategy-and-support.md`)

## 3. 架构数据流

```
push tag desktop-vX.Y.Z 或 workflow_dispatch
  ↓
desktop-release.yml matrix
  ├─ ubuntu-24.04 (default shell: bash) → cargo tauri build --bundles "$BUNDLES" ✓ 一直 OK
  └─ windows-latest (default shell: pwsh) → cargo tauri build --bundles "$BUNDLES"
                                              ↑
                                              pwsh 不识别 bash $VAR (pwsh 用 $env:VAR)
                                              → 实际执行 `--bundles ""` → clap "value required" fail
                                              ↓
                                              FIX: 加 shell: bash 强制走 Git Bash (Windows runner 预装)
```

## 4. Root cause

GH Actions log line 1430-1440 (`/tmp/win_log.txt`):

```
2026-05-25T05:43:47.5414364Z ##[group]Run cargo tauri build --bundles "$BUNDLES"
2026-05-25T05:43:47.5473258Z shell: C:\Program Files\PowerShell\7\pwsh.EXE -command ". '{0}'"
2026-05-25T05:43:47.5475604Z   BUNDLES: nsis,msi
2026-05-25T05:43:47.8632034Z error: a value is required for '--bundles [<BUNDLES>...]' but none was supplied
```

env 变量 `BUNDLES=nsis,msi` **已设置**, 但 pwsh 把 `$BUNDLES` 当作 PowerShell 变量(未定义), 替换为空串 → tauri 收到 `--bundles ""` → fail.

**回归引入 commit**: `ed151e1` (2026-05-22 "feat(release): Tauri auto-updater + package manager CI infra"):

```diff
-        run: cargo tauri build --bundles ${{ matrix.bundles }}
+        env:
+          BUNDLES: ${{ matrix.bundles }}
+        run: cargo tauri build --bundles "$BUNDLES"
```

把 GH Actions template 展开 (`${{ matrix.bundles }}` 在 YAML render 时替换成 `nsis,msi`) 改成 shell-runtime 展开 (`"$BUNDLES"` 依赖 shell 类型). Linux job 用 bash 默认通过, Windows job 用 pwsh 默认炸.

## 5. Fix

`.github/workflows/desktop-release.yml` "Build Tauri bundles" 步骤加 `shell: bash`:

```yaml
      - name: Build Tauri bundles
        working-directory: apps/attune-desktop
        shell: bash          # ← 新增, 强制 Git Bash (Windows runner 预装)
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
          BUNDLES: ${{ matrix.bundles }}
        run: cargo tauri build --bundles "$BUNDLES"
```

`shell: bash` 在 GH Actions 上跨平台:
- Linux/macOS → 系统 bash (与默认一致)
- Windows → `C:\Program Files\Git\bin\bash.exe` (actions/checkout 已用)

不影响 Linux job, 修复 Windows job.

## 6. 验证

无法本地复现 (Tauri Windows build 需要 windows-latest runner). 验证路径:

1. push fix 到 `develop` 分支
2. `workflow_dispatch` 触发 `desktop-release.yml` 指定 `ref: v1.0.0` 同时手动重 build
3. 观察 Windows job 进展到 "Build Tauri bundles" 后是否产出 `.exe` + `.msi`
4. `softprops/action-gh-release@v2` 步骤会自动把 Windows artifact 上传到现有 `desktop-v1.0.0` release page (action 默认 update 模式, 不覆盖已存在文件)

## 7. Re-trigger 路径(用户决策)

**Option A** (推荐): workflow_dispatch 重 build v1.0.0
```bash
gh workflow run desktop-release.yml --ref v1.0.0
```
但是: workflow_dispatch 跑时 checkout 的是 ref `v1.0.0` (tag 指向 commit), 那个 commit **没有本 fix**. 需要先 cherry-pick fix 到 v1.0.0 commit 重新打 tag, 或:

**Option B** (干净): 不动 v1.0.0 tag, 推 v1.0.1 (per `2026-05-26-v1-0-1-upgrade-strategy-and-support.md`) 时 Windows artifact 自然带出. v1.0.0 release page 保留 "仅 Linux" 状态, README 显式注明 "v1.0.0 Windows 用户请用 v1.0.1+ installer" (1-2 行).

**Option C** (临时): 在 develop 上推 fix, workflow_dispatch 触发(不 ref tag), 让 build 产 artifact 但不写 release(因 `if: startsWith(github.ref, 'refs/tags/desktop-v')` 跳过 release upload). artifact 仅保留为 GH Actions artifact 30 天, 手工下载后 `gh release upload desktop-v1.0.0 *.exe *.msi` 补到 v1.0.0 release.

**推荐**: **Option B** — 干净, 不破坏 v1.0.0 tag 不可变性, 配合本周 v1.0.1 推进自动覆盖.

## 8. 衔接

- v1.0.1 sprint (`2026-05-26-v1-0-1-upgrade-strategy-and-support.md`) push 时本 fix 已合 develop, 自动带出 Windows artifact
- 后续 desktop-vX.Y.Z* tag (含 alpha/beta/rc) 均自动 Windows + Linux 双轨

## 9. 关联规则

- CLAUDE.md "Git 分支管理标准": fix 进 develop, 不 cherry-pick 到 main (本 commit 不是 hotfix, 是 CI infra fix, 走 GitFlow Lite)
- CLAUDE.md "RC 阶段纪律 Gate 2": 本来 GA 前应跑 Windows build verify, 漏检根因是 desktop-v1.0.0-rc.* 中没有 Windows fail 信号 (rc 链可能未跑过 release workflow, 或 rc 时还没 ed151e1 改动). 后续 rc tag 必须 wait full matrix green 再升 GA.
- CLAUDE.md "Bug reproduce 第一步必须 user 视角": user 报告 "Windows 版本没有生成", 第一步就是 GH Actions log → root cause 定位, 符合 user-first.

## 10. Known limitations

- 本 fix 不补 v1.0.0 release page Windows artifact, 由 Option B (v1.0.1) 覆盖
- README 下载表 v1.0.0 行的 Windows 列需更新为 "用 v1.0.1+ installer" 或类似 (v1.0.1 sprint 时处理)

## 11. 风险登记

| 风险 | 影响 | 缓解 |
|------|------|------|
| `shell: bash` 在 Windows runner 不可用 | Windows build 又 fail | Git Bash 是 GH-hosted windows-latest 标配 (actions/checkout 已用), 极低风险 |
| Tauri 在 Git Bash 下 path 处理不同 (`\` vs `/`) | 产物路径异常 | tauri-cli 自身做 path normalize, 历史 `actions/setup-node` + `npm ci` 等也跑 bash 无问题 |
| v1.0.0 release page 永久缺 Windows artifact | user 体验差 | README 显式标注 + v1.0.1 快速跟上 |
