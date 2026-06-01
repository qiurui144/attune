# Reference WASM fixture (cross-platform agent distribution)

中性确定性 wasm agent,自证 attune-core 的 wasm runtime 链路(stdin JSON → 计算 →
stdout JSON → exit code)。**非 vertical 业务** — OSS 边界保持,attune-pro 的真实
agent 迁移按此契约各自产 `.wasm`(见 spec §10 / plan §5)。

## 文件

- `echo_calc_agent.rs` — 源码(整数计算 add/sub/mul + redline/trap/loop/bad-input 边界)
- `echo_calc_agent.wasm` — 预编译产物(**入库**,运行测试无需 wasm 工具链)

## 重新编译(仅当改 .rs 时)

```bash
rustup target add wasm32-wasip1   # 一次性
cd rust/crates/attune-core/tests/fixtures/wasm
rustc --target wasm32-wasip1 -O echo_calc_agent.rs -o echo_calc_agent.wasm
```

## 契约 (spec §5.2)

| op | 行为 | exit |
|----|------|------|
| add / sub / mul | 整数运算 → `{"ok":true,"result":{"value":N},...}` | 0 |
| redline | 业务红线 → red_lines_violated | 2 |
| trap | panic → wasm unreachable | 1(宿主映射) |
| loop | 死循环 → epoch timeout | -1(timed_out) |
| bad-input / 缺字段 | 输入非法 | 1 |

整数运算(无 f64)保证 wasm/native 输出逐字节一致(spec R2/IR3)。
