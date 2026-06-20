# OSS 导出引擎 (CAP-1) — 实现报告

**日期**: 2026-06-20  **分支**: `worktree-agent-a619ad7ca0a6a6260`  **base**: develop @ 6bfac00
**范围**: attune-core 新 `export/` 模块 + attune-server `/api/v1/export` 下载端点 + UI 下载按钮
**任务**: 文档参数差异→"输出表格并下载";参考标书→"可直接下载的 doc 或 pdf";"准确"输出。

---

## 1. Artifact IR(稳定 JSON 契约)

`attune_core::export`(`src/export/mod.rs`)。serde **邻接标签**(adjacent-tag)wire 形态,REST 友好:

```jsonc
{ "type": "table",    "data": { "title": "设备参数差异", "headers": [...], "rows": [[...]], "aligns": ["left","right","right"] } }
{ "type": "document", "data": { "title": "...", "blocks": [ {"kind":"heading","level":2,"text":"..."}, {"kind":"paragraph","text":"..."}, {"kind":"list","ordered":true,"items":[...]}, {"kind":"table", ...} ] } }
```

- `Table` = headers + rows + 每列 `Align`(left/center/right);`validate()` 强制行宽不变量(零列/参差行/aligns 长度错 → `malformed-ir`)。
- `Block` = Heading / Paragraph / List / Table。
- `ExportFormat` = xlsx | csv | md | docx | pdf;`cost_tier()` 恒为 `Free`,`extension()`/`mime()`/`parse()` 全套。
- `Artifact::render(format)` 是唯一入口(先 validate 再 render)。
- 错误码(kebab,稳定):`malformed-ir` / `unsupported-artifact` / `render-failed`,`http_status_for()` 映射 400/500。

## 2. 5 格式渲染器(纯 Rust,跨平台无系统依赖)

| 格式 | 库 | 说明 |
|------|----|------|
| **md** | 内部(零依赖) | GFM 表格(pipe/backslash 转义 + 对齐标记)、文档块 |
| **csv** | `csv` 1 | RFC-4180 + **公式注入转义** |
| **xlsx** | `rust_xlsxwriter` 0.95 | 一表一 sheet(多表文档→多 sheet);粗体表头;列对齐;sheet 名 31 字+非法字符净化 |
| **docx** | `docx-rs` 0.4 | 块→Word 段落/表;表头粗体;列表用 bullet/序号前缀(免 numbering.xml) |
| **pdf** | `typst-as-lib` 0.15 + `typst-pdf`/`typst` 0.14 | **`include_bytes!` 嵌 CJK 子集字体**;IR→typst markup(用户串全转义,防 typst 注入) |

**版本对齐坑**:typst-as-lib 0.15.5 内部 pin `typst 0.14`;最初装 0.15 触发 `PagedDocument: Document` trait-bound 不满足 → 全栈降 0.14;`PagedDocument` 在 0.14 走 `typst::layout::PagedDocument`(typst_library 再导出),无需单独 typst-layout dep。

**CJK 字体(spec R1 最大风险,已闭)**:`pyftsubset` 把 WenQuanYi Micro Hei(Apache-2.0)子集到 **CJK 常用块全集(U+4E00–9FFF)+ ASCII/Latin-1/CJK 标点**,产物 `assets/fonts/AttuneCJK-subset.ttf` = **2.98 MB**(全块保留=任意中文不丢字,非激进小子集),`include_bytes!` 进二进制,typst preamble `#set text(font: "WenQuanYi Micro Hei")`。`assets/fonts/LICENSE.md` 记许可与子集命令。

## 3. Round-trip 准确性实测(spec §9.1,最高价值 — "准确"硬证据)

`crates/attune-core/tests/export_roundtrip.rs`(9 测试全 PASS):**渲染→独立 reader 重解析→断言内容等于 IR**。

| 格式 | 重解析器 | 验证内容 | 结果 |
|------|---------|---------|------|
| csv | `csv` reader | 表头 + 每行精确等(含 CJK);公式注入单元回读带 `'` 前缀 | ✅ |
| xlsx | `calamine` | sheet 名(CJK)+ 每单元格精确(CJK/数字);多表→多 sheet | ✅ |
| docx | unzip `word/document.xml` | 标题/表头/单元格/列表项全部逐字存在(含 CJK) | ✅ |
| **pdf** | `pdf_extract::extract_text_from_mem` | **`pdf_chinese_roundtrip_not_garbled`**:`设备参数差异报告`/`额定功率`/`电压`/… 每条中文抽回**不乱码**;220V/1500瓦 数字存活 | ✅ |
| md | 字符串断言 | 表头分隔行 + 右对齐 `---:` 标记 + 每数据行 | ✅ |

**结论**:5 格式全部内容精确;**中文 PDF round-trip 抽回不乱码通过** = spec R1 最大风险关闭。

## 4. 下载端点 + UI

**端点** `crates/attune-server/src/routes/export.rs`:`POST /api/v1/export { artifact, format, filename? }` → 文件流下载。
- **零成本**:无会员门、无 LLM、无隐私出网;`vault_guard` bypass(渲染 client IR,不读 vault DEK,与 writing/documents 同)。
- **安全**:`download_filename()` 路径穿越净化(`../`/`..\`/控制字符→`_`,去前导点);`Content-Disposition` 双形态(ASCII fallback `filename=` + **RFC-5987 `filename*=UTF-8''`** 让 CJK 名安全)。
- 稳定 kebab 错误码:`unsupported-format` / `malformed-ir`。

**HTTP e2e** `crates/attune-server/tests/export_route_test.rs`(5 测试 PASS,真 router via `spawn_eval_server`):xlsx 下载 calamine 重解析 / **CJK pdf 下载 pdf-extract 抽回不乱码** / 恶意文件名 header 净化(无 `/`、无 `..`)/ 未知格式→400 / 畸形 IR→400。

**UI**:
- `hooks/useExport.ts` — POST→blob→anchor 点击下载;读 Content-Disposition 文件名(RFC-5987 decode);typed IR builders。
- `components/ExportButton.tsx` — 格式菜单(可窄化);a11y(role=menu/menuitem,aria-pressed);`data-testid`。
- `views/DocIntelView.tsx` 接线:综述报告→Document(md/docx/pdf),批注集→Table(xlsx/csv/md/pdf)。
- i18n zh/en parity(`export.*` + `docIntel.export.*`),**双 grep 守卫 0 输出**;`npm run build`(tsc --noEmit + vite)PASS;`dist/index.html` 已重建提交。

## 5. §6.1 六类测试矩阵

| 类别 | 覆盖 | 位置 |
|------|------|------|
| golden/happy | 设备差异表 5 格式 + 文档块 | roundtrip + 内联 |
| 属性测试(≥3) | 任意表→5 格式不 panic+可重解析 / IR json round-trip / **成本反偷跑(cost_tier 恒 Free)** | `export/tests.rs` proptests |
| 边界(≥5) | 空行表 / 超宽超高(40×200)/ unicode+emoji / 特殊字符 md 转义 / 深标题 clamp | `export/tests.rs` |
| 异常(≥3) | 参差行 / 零列 / 多表→csv / 无表→xlsx / aligns 长度错 | `export/tests.rs` |
| 集成 e2e(≥1) | HTTP 端到端下载 5 case | `export_route_test.rs` |
| 回归 | round-trip 准确性 9 case 永久 gate | `export_roundtrip.rs` |

**总计**:attune-core export 单元 28 + roundtrip 9 + server route 5 + route 单元 2 = **44 测试全 PASS**。

## 6. 安全

- **CSV/公式注入**:`=`/`+`/`-`/`@`/前导 tab/CR 开头单元 → 前缀 `'`(xlsx + csv 都过 `escape_cell`);proptest 注入危险前缀,e2e 回读断言中和。
- **路径穿越**:文件名 stem 净化(剥目录分量、控制/保留字符、前导点、长度 clamp);RFC-5987 编码;e2e 断言 header 无 `/`/`..`。
- **typst 注入**:PDF 渲染把用户串当 markup content 全转义(`#$*_<>@=…`),恶意单元不能注入 typst 指令。
- **成本契约**:`export` 模块**不 import `LlmProvider`**(编译期 no-LLM 守卫)+ proptest 断言 cost_tier 恒 Free → 导出永不静默升级为付费。

## 7. clippy + i18n

- `cargo clippy -p attune-core -p attune-server --all-targets -- -D warnings` **干净**(修 1 处 manual-pattern-char-comparison)。
- i18n 双守卫(硬编码 CJK / zh-en key parity)**0 输出**。
- 既有测试无回退(middleware 5 / export 全绿)。

## 8. 已知限制

- **PDF 排版**:typst 默认样式,无自定义页眉/页脚/封面/分页控制(MVP);后续可加 typst 模板参数。
- **docx 列表**:用文字 bullet/序号前缀(非原生 numbering.xml)——视觉正确、可编辑,但 Word 列表"增删自动重排序号"不可用。
- **xlsx 数字**:所有单元按字符串写(`write_string`)——保证 round-trip 字面精确,但不写成 Excel 数值类型(不参与公式)。这是**刻意**的(准确性 > 类型推断;且避免公式注入面)。
- **大表**:已测 40 列×200 行;未压测 10万行级(MVP 在线导出,非批量 ETL)。
- **字体覆盖**:子集含 CJK 常用块全集 + ASCII/Latin-1/CJK 标点;**未含**生僻扩展区(U+3400+ CJK-A、emoji 彩色字形)——这类字符 PDF 会落字(其余格式不受影响,因字体只用于 PDF)。
- typst/typst-pdf pin **0.14**(随 typst-as-lib 0.15.5);升级 typst-as-lib 时需同步。

## 9. commits(worktree 分支,未 push)

- `52c73dc` feat(export): artifact IR + 5 pure-Rust renderers
- `c51b8b8` test(export): §9.1 round-trip accuracy gate — incl CJK PDF un-garbled
- `8e30366` feat(export): POST /api/v1/export download endpoint + route e2e tests
- `5b745b9` feat(export-ui): ExportButton + download hook, wired into DocIntelView
