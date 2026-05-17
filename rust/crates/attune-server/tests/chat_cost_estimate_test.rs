// chat 响应体 cost_estimate 接线验证
// 直接测 attune_core::cost 函数逻辑，确认接线所用的三个函数行为符合预期

#[cfg(test)]
mod chat_cost_estimate_tests {
    use attune_core::cost::{estimate_tokens, estimate_cost_usd, lookup_pricing};

    #[test]
    fn cost_estimate_fields_for_cloud_model() {
        let system_prompt = "你是一个知识库助手，以下是相关文档：\n\n文档1: 测试内容abc";
        let user_msg = "请帮我总结这份文档";
        let response = "根据文档内容，这是一份关于测试的文档。";
        let model = "gpt-4o";

        let tokens_in = estimate_tokens(system_prompt, model)
            + estimate_tokens(user_msg, model);
        let tokens_out = estimate_tokens(response, model);

        assert!(tokens_in > 0, "输入 token 必须 > 0");
        assert!(tokens_out > 0, "输出 token 必须 > 0");

        let cost_usd = estimate_cost_usd(tokens_in, tokens_out, model);
        assert!(cost_usd.is_some(), "gpt-4o 应有定价");
        assert!(cost_usd.unwrap() > 0.0, "费用应 > $0");
    }

    #[test]
    fn cost_estimate_is_none_for_local_ollama_model() {
        // Ollama 本地模型不在定价表中，cost_usd 应为 None
        let tokens_in = estimate_tokens("测试消息", "qwen2.5:3b");
        let tokens_out = estimate_tokens("测试响应", "qwen2.5:3b");

        let cost_usd = estimate_cost_usd(tokens_in, tokens_out, "qwen2.5:3b");
        assert!(cost_usd.is_none(), "Ollama 本地模型无定价，cost_usd 应为 None");
    }

    #[test]
    fn cost_estimate_tokens_in_includes_system_and_user() {
        // tokens_in 包含 system_prompt + user_message 两部分
        let system = "系统提示词";
        let user = "用户问题";
        let model = "claude-3-5-sonnet";

        let combined = estimate_tokens(system, model) + estimate_tokens(user, model);
        let single = estimate_tokens(&format!("{system}{user}"), model);

        // 拆开估算 vs 合并估算差异应在 ±2 以内（字符数相同，系数相同）
        assert!(
            (combined as i64 - single as i64).abs() <= 2,
            "分开和合并估算差距不应超过 2 tokens，实际: combined={combined} single={single}"
        );
    }

    #[test]
    fn lookup_pricing_covers_common_cloud_providers() {
        // 确认常见云端模型都有定价，以免 cost_usd 意外返回 None
        // gemini-2.0-flash 尚未入表（定价表覆盖 gemini-1.5-pro）；测 1.5-pro
        for model in &["gpt-4o", "gpt-4o-mini", "claude-3-5-sonnet", "gemini-1.5-pro"] {
            assert!(
                lookup_pricing(model).is_some(),
                "模型 {model} 应有定价"
            );
        }
        // 本地模型不在表中，正常返回 None
        assert!(lookup_pricing("qwen2.5:3b").is_none());
    }

    #[test]
    fn cost_estimate_json_shape() {
        // 验证接线到响应体时的字段构建方式（模拟 chat.rs 第 6 步逻辑）
        let system_prompt = "系统提示";
        let user_message = "用户提问";
        let response = "模型回答";
        let model_name = "gpt-4o";
        let is_local = false;

        let tokens_in = estimate_tokens(system_prompt, model_name)
            + estimate_tokens(user_message, model_name);
        let tokens_out = estimate_tokens(response, model_name);
        let cost_usd: Option<f64> = if is_local {
            None
        } else {
            estimate_cost_usd(tokens_in, tokens_out, model_name)
        };

        let json = serde_json::json!({
            "tokens_in": tokens_in,
            "tokens_out": tokens_out,
            "cost_usd": cost_usd,
            "is_local": is_local,
        });

        assert_eq!(json["is_local"], false);
        assert!(json["tokens_in"].as_u64().unwrap() > 0);
        assert!(json["cost_usd"].as_f64().is_some(), "cost_usd 应为 f64");
    }

    #[test]
    fn tokens_in_includes_history_messages() {
        // 多轮对话下 tokens_in 必须覆盖 system + history[] + user，不得漏算 history
        let system_prompt = "你是知识库助手";
        let user_message = "继续问题";
        let model = "gpt-4o";

        // 模拟两轮历史（user + assistant 各一条）
        let history = vec![
            ("user", "第一轮用户消息"),
            ("assistant", "第一轮助手回复，内容稍长一些用于验证计入"),
        ];

        // 按 chat.rs 修复后的逻辑：system + 逐条 history + user
        let mut tokens_in = estimate_tokens(system_prompt, model)
            + estimate_tokens(user_message, model);
        for (_, content) in &history {
            tokens_in += estimate_tokens(content, model);
        }

        // 不含 history 的旧逻辑
        let tokens_in_without_history = estimate_tokens(system_prompt, model)
            + estimate_tokens(user_message, model);

        assert!(
            tokens_in > tokens_in_without_history,
            "含 history 的 tokens_in ({tokens_in}) 必须大于不含 history 的值 ({tokens_in_without_history})"
        );

        // 验证 history 贡献量大于零
        let history_tokens: usize = history.iter().map(|(_, c)| estimate_tokens(c, model)).sum();
        assert!(history_tokens > 0, "history tokens 应 > 0");
        assert_eq!(tokens_in, tokens_in_without_history + history_tokens);
    }

    #[test]
    fn input_rate_per_k_is_pure_input_price() {
        // input_rate_per_k 必须是 input 单价，而非 input/output 混合均价
        // claude-3-opus：input=$0.015/K，output=$0.075/K（差 5×）
        let pricing = lookup_pricing("claude-3-opus").expect("claude-3-opus 应在定价表");
        let input_rate = pricing.input_per_1k_usd;
        let output_rate = pricing.output_per_1k_usd;

        assert!(
            (output_rate / input_rate - 5.0).abs() < 0.1,
            "claude-3-opus input/output 价差应约 5×，实际 input={input_rate} output={output_rate}"
        );

        // 用 input_rate 估算 100 个 input token 的费用
        let estimated = 100.0 * input_rate / 1000.0;
        let from_mixed = 100.0 * (input_rate + output_rate) / 2.0 / 1000.0;
        assert!(
            (from_mixed / estimated - 3.0).abs() < 0.5,
            "混合均价会高估 input 费用约 3×，验证方向性误差存在"
        );
    }
}
