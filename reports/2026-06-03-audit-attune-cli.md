# 代码审计报告: attune-cli

- **审计对象**: `rust/crates/attune-cli` (attune, Rust)
- **范围**: 单文件 `src/main.rs` (1838 LOC) + `tests/` (4 smoke test files, 390 LOC)
- **日期**: 2026-06-03
- **方法**: 全文件读取 (1838 行,小区可较全) + grep 验证 (panic/exit/mixed-lang/doc-drift)

---

## Scorecard

| 维度 | 分 (1差5优) | 依据 |
|------|------|------|
| code_quality | 4 | 错误处理基本统一走 `VaultError`/`Result`,exit-code 协议 (D5.7) 有文档 + 测试。扣分:`process::exit` 与 `Result` 返回两条错误路径混用 (11 处直接 exit);2 处 `.unwrap()` 在动态 JSON 序列化上 |
| complexity | 3 | 单文件 1838 LOC,`run()` 调度器 ~350 行 (L402-754) 含两段大内联 handler (Ocr ~66 行、Transcribe ~55 行)。无深嵌套但 dispatch 逻辑分散在「头部 early-return match」+「post-open match」两处,且两处 unreachable 互相镜像 |
| simplification_potential | 3 | 可拆为多 module + 抽公共 helper,净减 ~80-120 LOC + 大幅降低单文件认知负荷。重复的 `.map_err(VaultError::Io)` / `data_dir().join(name)` copy-loop 在 export/import 重复 |
| doc_accuracy | 3 | exit-code doctoc 与实现一致 (测试锁定);但 `min_core_version="0.4.0"` 硬编码已 drift (crate 现 1.2.0);RELEASE.md/DEVELOP.md **完全没有** CLI 子命令文档 (28 个子命令零文档覆盖) |

---

## 分维度 Findings

### (1) 正确性 / silent failure

| Sev | 位置 | 一句话 |
|-----|------|--------|
| Low | `main.rs:534` `main.rs:588` | OCR/ASR JSON envelope 用 `serde_json::to_string_pretty(&envelope).unwrap()` — envelope 含用户 OCR 文本 (任意 bytes 经 `out.text`),理论可序列化但 `.unwrap()` 是 panic 路径而非 exit-code-3;应 `?` 或 `.map_err` 走 engine-failure(3) |
| Low | `main.rs:1221` | `serde_json::from_str(&s).unwrap_or_default()` — folder-links.json 损坏时静默丢弃全部历史 link(回退空 Vec),用户已有 link 无声消失;至少应 eprintln 警告 |
| Low | `main.rs:1235` | `serde_json::to_string_pretty(&links).expect("ser")` — `.expect("ser")` panic 而非 Result;FolderLink 必可序列化,风险低但破坏统一错误路径 |
| Info | `main.rs:1333` | multipart `Part::...mime_str(...).unwrap()` — 静态字符串 mime,不会 fail,可接受 |
| Info | `main.rs:396-399` | `main()` 仅 OCR/ASR 之外的错误映射 exit-code;OCR/ASR 内部多处直接构造 `VaultError` 经 `run()` 返回 → `classify_error_exit_code` 映射正确,一致 |

### (2) 复杂度热点

| Sev | 位置 | 一句话 |
|-----|------|--------|
| Med | `main.rs:402-754` `run()` | 350+ 行单函数:头部「vault-free early-return」大 match (L404-471) + 两段内联 handler (Ocr L473-539 / Transcribe L540-595) + post-open match (L599-752),阅读需在两个镜像 match 间跳转 |
| Med | `main.rs:473-539` Ocr inline | 66 行内联 OCR 逻辑 (pre-validate + provider detect + structured extract + bbox 裁剪 + envelope 组装) 应抽成 `run_ocr(...)` 与其他 `run_*` 一致 |
| Med | `main.rs:540-595` Transcribe inline | 55 行内联,同上,应抽 `run_transcribe(...)` |
| Low | `main.rs:1-366` clap 定义 | 366 行纯 derive enum (28 个 `Commands` variant + `AgentAction` + `FlowAction`),doc-comment 占大头;可单独 `cli.rs` module |

### (3) Dead code / 未用

| Sev | 位置 | 一句话 |
|-----|------|--------|
| Low | `main.rs:551-556` Transcribe `--no-wait` | `wait` flag 唯一作用是 `!wait` 时打 WARN 后忽略 — 「reserved for future」死参数,当前对行为零影响 |
| Low | `main.rs:310-314` AgentAction::Tune `dry_run` | `--dry-run` default true,`=false` 直接 refuse (L1612);即「只能 true」的参数,实际是占位 |
| Info | exit-code `2`/`4` | 协议文档 (L368-380) 列 code 2 (red-line) / 4 (network),`classify_error_exit_code` 从不返回 2 或 4(仅 Deploy L695/704 直接 exit 2)— 4 完全未用,文档应标 reserved |

### (4) 简化 / 压缩机会

| 机会 | 位置 | 估省 LOC |
|------|------|---------|
| 抽 `copy_vault_files(src,dst,&names)` helper | export L668-683 + import L1739-1753 两处近乎逐字重复 4-file copy-loop + tantivy 递归 | ~15 |
| 抽 `run_ocr()` / `run_transcribe()` 出 `run()` | L473-595 内联 → 函数 | 0 净(搬移),但 `run()` -120 行,可读性大升 |
| 统一 vault-free dispatch 表 | L404-471 + L724-733 + L736-751 三处 unreachable 互镜像;若改用「先 dispatch vault-free,再 open vault」单层结构可删两段 unreachable arm | ~25 |
| 抽 `locate_or_err(name, flag_hint)` | `run_agent_gate`/`registry`/`flow`/`tune` 各自重复「match Some(p)/None=>locate_named().ok_or_else(NotFound(...))」4 次 | ~20 |
| 拆分 module: `cli.rs`(clap)+`plugin.rs`+`agent.rs`+`vault_io.rs`+`ocr_profile.rs` | 单 1838 LOC → 5-6 个 ~200-400 LOC 文件 | 0 净,认知负荷大降 |

**净可删/合并估计**: ~60-80 LOC(helper 抽取)+ module 拆分(0 净 LOC 但单文件认知负荷显著下降)。

### (5) Doc-drift 清单

| Sev | 位置 | 漂移 |
|-----|------|------|
| Med | `main.rs:1336` | `min_core_version="0.4.0"` 硬编码,crate `Cargo.toml` 已 v1.2.0 — publish 的 plugin 永远声明兼容 ≥0.4.0,与实际 core 版本脱节;应读 workspace version 或 const |
| Med | RELEASE.md / DEVELOP.md | 28 个 CLI 子命令(ocr/transcribe/agent/plugin-*/deploy/vault-*/rollback 等)**零文档**;grep `attune (ocr\|transcribe\|agent\|plugin\|deploy)` 在 RELEASE.md/DEVELOP.md 均 0 命中。CLAUDE.md §1.1.2 要求 user-facing CLI 进 README/DEVELOP |
| Low | `main.rs:368-380` exit-code 表 | 文档列 code 4「network unreachable (reserved for future REST mode)」但代码无任何路径产出 4;reserved 应明示「not yet emitted」 |
| Low | `main.rs:79` Transcribe `--wait` doc | doc 说「default true; flag reserved for future async」,但 flag 当前完全 no-op(仅 WARN),与「reserved」措辞一致但易误导用户以为有效 |

### (6) 安全 (§1.4 secrets / 注入)

| Sev | 位置 | 一句话 |
|-----|------|--------|
| Info | `main.rs:915-919` PluginKeygen | 私钥可打 stdout(`PRIVATE_KEY=...`)— 有 ⚠️ 提示「save offline + clear shell history」,默认推荐 `--out-priv` 写文件 chmod 600;符合 §1.4(无硬编码,提示得当) |
| Info | `main.rs:1308 1340` | admin_token 经 env `PLUGINHUB_ADMIN_TOKEN` / `ATTUNE_PLUGIN_KEY` / `ATTUNE_PLUGIN_SIGN_KEY` 读取,从不硬编码,从不 echo token 本体 — 合规 |
| Low | `main.rs:1274-1278` tar subprocess | `tar czf <pkg> -C <parent> <name>`,路径经 clap PathBuf,无 shell 拼接(用 `Command::args` 非 shell)→ 无注入;`unwrap_or(".")` / `unwrap_or_default()` 对畸形路径降级合理 |
| Info | `main.rs:1291` | `redirect::Policy::none()` 防 SSRF/重定向到内网 — 良好实践 |
| Info | mixed-lang | 121 行含 CJK user-facing 输出(eprintln/println 中文);CLI 是 dev/运维面非 Web UI,§i18n 守卫不覆盖,但与产品「user-facing 默认英文」精神不一致,属技术债非违规 |

### (7) 依赖冗余

| Sev | 一句话 |
|-----|--------|
| Info | Cargo.toml 依赖精简,无冗余:`strsim`(typo 建议,单用途 L775)、`tempfile`(plugin-publish tar 暂存 L1272)、`hex`(keygen)、`reqwest blocking`(login/sync/publish) 各有实使用点。无未用 dep |

---

## 总结

健康的单文件 CLI,错误处理与 exit-code 协议是亮点(D5.7 有文档 + 单测锁定)。主要问题是 **1838 LOC 单文件 + `run()` 350 行调度器** 的复杂度,以及 **CLI 子命令零文档** 的 doc-drift。无 P0/P1 正确性 bug。

**最大简化机会**: module 拆分(cli/plugin/agent/vault_io/ocr_profile)+ 抽 `run_ocr`/`run_transcribe`/`copy_vault_files`/`locate_or_err` helper,净减 ~60-80 LOC,单文件认知负荷显著下降。

**优先修**: (a) `min_core_version="0.4.0"` 硬编码 drift; (b) RELEASE/DEVELOP 补 CLI 子命令文档; (c) L534/588/1235 `.unwrap()/.expect()` 改走 Result 统一错误路径。
