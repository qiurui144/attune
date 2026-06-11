//! Reference wasm agent fixture — 中性确定性计算(非 vertical 业务,per plan §5)。
//!
//! 自证 wasm lane 链路:stdin JSON → 整数计算 → stdout JSON → exit code 契约。
//! **整数运算**(无 f64,per spec R2/IR3 浮点确定性约束)→ wasm/native 逐字节一致。
//!
//! 输入 (stdin, JSON 子集,手写极简 parser 避免依赖):
//!   {"a": <int>, "b": <int>, "op": "<add|sub|mul>"}
//! 特殊 op(边界/错误/超时测试):
//!   "redline"   → proc_exit(2)  (业务红线,exit code 2)
//!   "trap"      → unreachable    (wasm trap → 宿主映射 exit 1)
//!   "loop"      → 死循环          (epoch timeout → timed_out)
//!   "bad-input" → exit 1         (输入非法)
//!
//! 输出 (stdout, JSON):
//!   {"ok":true,"result":{"value":<int>},"audit_trail":["op=<op>"],"red_lines_violated":[]}
//!
//! 编译: `rustc --target wasm32-wasip1 -O echo_calc_agent.rs -o echo_calc_agent.wasm`
//! (CI / build 脚本入库预编译 .wasm,运行测试无需 wasm 工具链)。

use std::io::{Read, Write};

fn main() {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        std::process::exit(1);
    }

    let op = extract_str(&input, "op").unwrap_or_default();

    match op.as_str() {
        "redline" => {
            // 业务红线:stdout 给出 red_lines_violated,exit 2
            let out = r#"{"ok":false,"result":{},"audit_trail":["op=redline"],"red_lines_violated":["hard_red_line_triggered"]}"#;
            let _ = std::io::stdout().write_all(out.as_bytes());
            std::process::exit(2);
        }
        "trap" => {
            // 主动制造 trap(panic → wasm unreachable)
            panic!("intentional trap for test");
        }
        "loop" => {
            // 死循环,供 epoch timeout 测试。
            // WHY 有副作用:LLVM 视无副作用无限循环为 UB 会优化成 unreachable,
            // 故每轮做一个 volatile 写 + 偶发 flush(observable side effect),
            // 保证循环不被优化掉,从而真正撞 epoch deadline。
            let mut sink: u64 = 0;
            let p = &mut sink as *mut u64;
            loop {
                unsafe {
                    let v = std::ptr::read_volatile(p);
                    std::ptr::write_volatile(p, v.wrapping_add(1));
                }
            }
        }
        "add" | "sub" | "mul" => {
            let a = match extract_int(&input, "a") {
                Some(v) => v,
                None => exit_bad_input(),
            };
            let b = match extract_int(&input, "b") {
                Some(v) => v,
                None => exit_bad_input(),
            };
            let value: i64 = match op.as_str() {
                "add" => a.wrapping_add(b),
                "sub" => a.wrapping_sub(b),
                "mul" => a.wrapping_mul(b),
                _ => unreachable!(),
            };
            let out = format!(
                r#"{{"ok":true,"result":{{"value":{value}}},"audit_trail":["op={op}"],"red_lines_violated":[]}}"#
            );
            let _ = std::io::stdout().write_all(out.as_bytes());
            std::process::exit(0);
        }
        _ => exit_bad_input(),
    }
}

fn exit_bad_input() -> ! {
    let _ = std::io::stderr().write_all(b"bad-input: missing or invalid op/a/b");
    std::process::exit(1);
}

/// 极简 JSON 字段提取:找 `"key"` 后第一个 `:`,跳空白,读到下一个 `,` 或 `}`。
/// 仅用于本 fixture 的扁平 JSON(无嵌套 / 无转义需求)。
fn extract_raw<'a>(input: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\"");
    let idx = input.find(&pat)?;
    let after = &input[idx + pat.len()..];
    let colon = after.find(':')?;
    let val = after[colon + 1..].trim_start();
    let end = val
        .find(|c: char| c == ',' || c == '}')
        .unwrap_or(val.len());
    Some(val[..end].trim())
}

fn extract_int(input: &str, key: &str) -> Option<i64> {
    extract_raw(input, key)?.parse::<i64>().ok()
}

fn extract_str(input: &str, key: &str) -> Option<String> {
    let raw = extract_raw(input, key)?;
    Some(raw.trim_matches('"').to_string())
}
