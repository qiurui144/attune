# doc_privacy 出网点接线 — INT-2 export-bypass 闭洞

**Date**: 2026-06-21  **Branch**: agent-a74161b79760c07ab (worktree off develop)
**Tip**: `8a2494c` (3 commits on top of develop `f948fe7`)
**Task**: #134 doc_privacy 补全:出网点接线 + PDF + REST/UI

## 背景

`doc_privacy` 已落 G4 机密 fail-closed + G5 分级→OutboundGate + 可逆 `[KIND_N]`
字节脱敏 + `enforce_file_egress`（单一文件出网门），但 `enforce_file_egress`
**未接进真实出网点** —— "文档导出绕脱敏"洞只闭在 helper 层。

## 真出网点盘点（关键判定）

逐一审计 attune 的"文件字节离开设备"路径：

| 路径 | 性质 | 是否 plaintext-doc 出网 | 处置 |
|------|------|------------------------|------|
| **`POST /api/v1/export`**（export.rs） | 渲染 `Artifact` IR（来自解密 vault 内容 + LLM 产出）→ 下载 office 文件 | ✅ **是**（明文文档字节作为下载离开） | **已接门** |
| **skill-runtime `/run` export step** | 同上，产出交付物 base64 内联返回 | ✅ **是** | **已接门** |
| `sync/webdav.rs` 备份上传 | 上传**加密 vault 快照**（sqlite+index tarball，AES 静态加密） | ❌ 否（已加密，非明文文档） | 不需接（加密层已护） |
| `ingest_webdav.rs` / `scanner_webdav.rs` | **入网**（远端文件 → vault），非出网 | ❌ 否（方向相反） | N/A |
| "分享链接" | 产品无此功能（grep 仅误命中 sharedStrings/shareable 等） | — | 不存在 |

→ **唯一 plaintext-doc 出网面 = export/skill-runtime 渲染下载路径**，已全部接门。

## 实现

### 1. core: `enforce_artifact_egress`（commit `aa6787c`）

`export` 引擎在 `Artifact` IR 层工作，故在 IR 层接门最干净：渲染**前**对 IR 的
所有文本字段（表头/单元格/文档块/列表项/嵌套表）扫描分级 + 就地脱敏。
- 机密标记（绝密/机密/confidential…，+ pro 插件可扩展 `extra_keywords`）→
  `Blocked`（fail-closed，文件从不渲染）；
- 否则用 `pii::Redactor::redact_batch`（全局唯一 `[KIND_N]`）就地脱敏，渲染出的
  文件不含明文 PII。默认 `Reversible`（可 `restore()`），`Irreversible` 供不可信分享。
- `collect_artifact_strings` / `apply_redacted_strings` 严格同序，保证脱敏串回写到
  正确槽位（含嵌套块）。7 个单测全过。

### 2. server 接线（commit `da87a79`）

- **`/export`**：机密 → 422 `doc-classified`（含 hint 建议改导 docx/txt）；PII → 渲染前
  脱敏。读 settings `privacy.export_confidential_keywords`（pro 行业标记）。
- **skill-runtime `/run`**：产出 artifact 过门；脱敏后**重渲染**脱敏 IR（重渲染失败也
  fail-closed，绝不回退到明文字节）。
- **REST 新增**：`POST /doc-privacy/scan`（分类文本，不泄 PII 值）+
  `/doc-privacy/export-preview`（dry-run 门，UI 下载前预警）。vault_guard bypass（纯扫描无 DEK）。

### 3. UI（commit `8a2494c`）

PrivacyView 加"文档导出保护"面板：密级检测在线 / 机密拦截(fail-closed) / 导出脱敏 /
PDF 仅拦截不逐字节脱敏 的提示。7 个 i18n key zh+en 双写，两守卫干净。

## PDF 决策（务实）

PDF 字节级逐字节脱敏 **保留 out-of-scope**（font/encoding map 改写风险高，
可能产出半脱敏/乱码文件）。改为：**含机密的 PDF 在 export 门 fail-closed 拒绝**
（422），hint 明示"改导出脱敏后的 docx/txt"。已对 5 格式（docx/pdf/md/csv/xlsx）
全部验证机密拦截生效。`redact_bytes` 的 PDF→`UnsupportedFormat` fail-closed 行为保持。

## 安全对抗结论（§6.1 + 真路由）

`export_route_test.rs`（真 router + 真下载，10/10 PASS）：
- **机密真被拦**：confidential artifact 经真 `/export` → **422 `doc-classified`，跨
  docx/pdf/md/csv/xlsx 全格式无文件产出**（`export_classified_artifact_blocked_fail_closed`）。
- **导出无明文**：PII artifact 经真 `/export` md+docx 下载 → 解出字节扫描，
  `13800138000` / `zhangsan@example.com` / `13900139000` **零明文**；docx 解 zip 看
  `word/document.xml` 同样零明文（`export_pii_artifact_downloads_redacted_no_plaintext`）。
- **可逆 round-trip**：core 单测 `restore()` 从 md 恢复全部原值。
- 干净文档不误伤（`export_clean_artifact_unchanged`）；preview 与真 export 判决一致；
  scan 不回显 PII 值。

## 六类测试覆盖

| 类 | 证据 |
|----|------|
| happy | normal artifact 导出脱敏成功 + 干净文档原样 |
| edge | 空文档 / 嵌套块字段序 round-trip / 表格多行 |
| error | 机密 422 / 重渲染失败 fail-closed / corrupt 不 panic（既有 redactor 测试） |
| adversarial | 机密跨 5 格式拦截 + docx zip 内明文扫描 + scan 不泄 PII |
| 并发 | N/A（纯函数，无共享态）|
| 资源 | 大文档批量 redact_batch（既有引擎，单 join） |

## clippy + i18n

- `cargo clippy -p attune-core -p attune-server --all-targets -- -D warnings` → 干净。
- i18n 双守卫（硬编码 CJK / zh-en key parity）→ 均无输出。tsc + vite build OK，
  dist/index.html 已重建并含新串。

## 测试结果

- `attune-core --lib doc_privacy`: **40 passed**（含 7 新 artifact-egress）。
- `attune-server --test export_route_test`: **10 passed**（含 5 新隐私/对抗）。
- `attune-server --lib skill_runtime / privacy`: 8 passed（无回归）。

## 残留

- PDF 逐字节脱敏未做（务实决策：fail-closed + 替代格式建议，已是可接受闭环）。
- `export_confidential_keywords` settings 的 pro 写入端 + UI 编辑暂未做（OSS 默认
  generic 集已生效；pro 插件可经 settings 注入，已留契约 + core 测试覆盖）。
- 真桌面 Playwright 验收（PrivacyView 面板渲染）未做（本 agent 无 GUI 起服；
  dist 已 grep 验证含新串）。
