# Agent 跨平台分发(WASM runtime)实施报告

> 日期:2026-06-01 · worktree:`agent-a3e3d1f0be6d1eb6f`(branch `worktree-agent-a3e3d1f0be6d1eb6f`)
> spec:`docs/superpowers/specs/2026-05-31-agent-cross-platform-distribution.md`
> plan:`docs/superpowers/plans/2026-05-31-agent-cross-platform-distribution.md`
> base:基于 `develop`(spec/plan + settings-validation 都在 develop;worktree 原 HEAD 在 main 落后 13 commit,已 `reset --hard develop` 对齐)

## 工具链状态

- ✅ `rustup target add wasm32-wasip1` **安装成功**(D3 未阻塞)。
- ✅ wasmtime 45.0.0 + wasmtime-wasi 45.0.0 依赖解析 + 编译通过。
- 盘:全程 /data > 190G(绿)、/ ~46G(> 15G 线,未触红)。

## 阶段 + commit SHA + 测试

| 阶段 | commit | 内容 | 测试结果 |
|------|--------|------|---------|
| D1 | `a32c31b` | `version.rs`:ATTUNE_VERSION + is_compatible(semver) | 6 单测 PASS |
| D1 | `1190a50` | `PluginManifest.min_attune_version`(Option,向后兼容) | — |
| D1 | `f37e18f` | `scan` version gate(skip 不兼容 + `[incompatible]`/`[invalid-min-version]`) | 4 单测 PASS |
| D1 | `bd5f2ee` | marketplace 返回 `plugin-incompatible-version`(409) | 编译通过 |
| D2 | `062e76e` | SkillSpec/AgentSpec 增 `wasm`/`wasi_caps` + 加载期校验(`wasm-entry-missing` / wasi_caps 白名单) | 5 单测 PASS |
| D2 | `f9b0c00` | `CapabilityRuntime` enum + `parse_runtime` + `resolve_wasm` + `dispatch_capability` 分流(python_subprocess/unknown→`unsupported-runtime`) | 7 单测 PASS |
| D2 | `8ddcc70` | `agent_runner` 走 `dispatch_capability`(契约不变) | 14 现有单测 PASS(无回归) |
| D3 | `7a2f2b8` | Cargo:wasmtime + wasmtime-wasi(`wasm-runtime` feature,默认开;`--no-default-features` 可关已验证) | 编译通过 |
| D3 | `c68dfd5` | `wasm_runtime.rs`:WasmRunner(Engine 复用 + per-call Store + WASI p1 + epoch deadline + StoreLimits 256MB + wasi_caps→WasiCtx) | 见 D4 gate |
| D4 | `8ae044c` | reference wasm fixture(`echo_calc_agent.wasm` 入库)+ `wasm_capability_gate.rs` | **15 测试 PASS(连跑 2 次稳定)** |
| D4 | `9fe9270` | CI:rust-test 矩阵(ubuntu+windows)加 wasm gate 步骤 + `--no-default-features` build | CI 配置(未触发真跑) |
| D5 | `e4f0ae6` | RELEASE.md(Highlights + Known Limitations)+ DEVELOP.md runtime 契约 SSOT | 文档 |

### wasm_capability_gate.rs 六类下限(全 PASS)

- **golden diff=0**:10 case wasm 输出逐字节 == 独立 native GT(整数运算)+ 1 sentinel。
- **proptest ×3**:随机 add/mul 与 native 一致;任意 i64 exit ∈ {-1..=2} 不 panic 宿主。
- **边界 ×5**:空 stdin / 10MB stdin / loop epoch timeout / 缺失 .wasm / 非法 module。
- **错误 ×3+**:redline(exit 2)/ trap(exit 1)/ bad-input(exit 1)/ unknown op。
- **E2E ×2**:真 plugin(含 .wasm)→ scan(version gate 放行)→ `agent_runner` dispatch 得 42;
  `min=99.0.0` 包被 scan 拒(`[incompatible]`)。

### test-fix-verify(per Agent 验证铁律)

1. fixture `loop` 被 LLVM 优化成 unreachable(无副作用无限循环=UB)→ 改 `volatile` 读写。
2. epoch timeout 分类靠 wall-clock 在并行负载下 **flaky** → 改 `Trap::Interrupt` downcast(稳定,wall-clock 仅兜底)。

## 已定决策落实

- wasmtime 嵌入 + 默认开 `wasm-runtime` feature(可 `--no-default-features` 关)✓
- WASI preview1(最小可用)✓ · epoch-based 超时**不引 tokio**(std::thread ticker)✓
- 复用 `binary` 字段 + `runtime` 判别(决策 D-a,`CapabilityInvocation.binary` 复用为执行体路径)✓
- `scan` 返回类型不变,字符串前缀 `[incompatible]`(决策 D-b)✓
- 新建 `version.rs` + `wasm_runtime.rs`(确认不存在)✓
- wasm 复用 `CapabilityResult` + exit 0/1/2/-1 契约(调用方无感)✓
- reference wasm fixture **不依赖 attune-pro 产物**(中性整数计算)✓

## 阻塞项

- 无。D3 工具链装成,全程未触阻塞。

## attune-pro 待迁移清单(跨仓,**不在本 worktree 做**)

attune-pro 仓需按本契约把确定性 agent 编译 `wasm32-wasip1` + 填 manifest(`runtime: wasm` +
`wasm:` + `wasi_caps` + plugin 级 `min_attune_version: "1.1.0"`),各 vertical 跑其
`agent_golden_gate` golden diff=0 才合入:

- **law-pro**:本息/利率/期限计算类(loan_terms / civil_loan 计算)、案号结构化(case_no)、
  时效计算(limitation)、银行流水聚合(bank_aggregator 纯计算部分)、证据链结构化(evidence_chain)。
- **patent-pro**:权利要求结构化抽取(patent_claims)。
- **保留 `rust_binary`**(native-only,不迁):依赖 poppler/OCR 预处理、系统 Chrome(chromiumoxide)、
  重 native 性能的 cap。
- **`data_only` 候选**:无计算逻辑、逻辑全在 prompt/LLM 的 agent。
- 强配对:attune-pro 用 wasm 的 plugin.yaml 填 `min_attune_version: "1.1.0"`。

迁移不阻塞 OSS ship(OSS reference fixture 已自证链路)。

## 未 push / 未动 develop

- 全部 commit 落在隔离 worktree branch `worktree-agent-a3e3d1f0be6d1eb6f`(12 实施 commit,基于 develop)。
- 未 push、未改 develop/main。由上层 merge。
