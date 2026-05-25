# Attune 隐私模型

> Attune 不假装"100% 全本地"——RAG 时云 LLM 必然要看到检索 chunk。  
> 但 Attune **诚实标注每一刻边界**：什么出网了、脱敏到什么程度、能不能审计、能不能撤回。  
> 这是 v0.6.0 Phase A.5 的核心交付。

## 三层隐私模型

```
┌──────────────────────────────────────────────────────────────┐
│ L0 🔒  per-file 标记                                          │
│        chunk 永不出现在云 LLM context                          │
│        强制本地 LLM (Ollama qwen2.5:3b 等)                     │
│        适用：核心案件 / 病历 / 工资条 / API key                │
│                                                              │
│ L1 🛡️  默认（OSS 免费层）                                     │
│        12 类格式化 PII 自动检测 + reversible placeholder        │
│        出网审计日志 + CSV 导出                                  │
│        适用：普通用户日常 RAG                                  │
│                                                              │
│ L3 🔐  LLM 语义脱敏（v0.7 Tier T3+/K3）                       │
│        chinese-roberta NER 识别人名/地名/化名/项目代号          │
│        识别"上次会议提到的客户"等隐含指代                       │
│        适用：律所核心业务 / 政企合规                            │
└──────────────────────────────────────────────────────────────┘
```

## L1 默认：12 类格式化 PII

| 类别 | 检测器 | 示例 | placeholder |
|------|--------|------|-------------|
| 身份证 | ISO 7064 mod 11-2 校验 | 11010119900307125X | `[ID_1]` |
| 手机 | 中国 11 位 + 可选 +86 | +8613812345678 | `[PHONE_1]` |
| 邮箱 | RFC 5322 | a@b.com | `[EMAIL_1]` |
| IPv4/IPv6 | 各段范围校验 + Ipv6Addr 解析 | 192.168.1.1 / 2001:db8::1 | `[IP_1]` |
| 信用卡 | Luhn 校验 | 4111 1111 1111 1111 | `[CARD_1]` |
| 银行卡 | 16-19 位 + 边界 | 6225881234567890 | `[CARD_1]` |
| API Key | 8 家前缀 (sk-/ghp_/AKIA/glpat/xoxb/hf_/AIza/sk-ant) | sk-abc...xyz | `[APIKEY_1]` |
| URL | https?:// | https://x.com/y | `[URL_1]` |
| MAC | xx:xx:xx:xx:xx:xx | aa:bb:cc:dd:ee:ff | `[MAC_1]` |
| 车牌 | 中国油车 + 新能源 | 京A12345 / 沪AD12345 | `[PLATE_1]` |
| GPS 经纬度 | 范围校验 | 39.9, 116.4 | `[GPS_1]` |

**插件提供的行业 PII**（在 plugin.yaml 声明）：
- `law-pro`: 案号 (2023)京01民终123号
- `medical-pro`: 病历号 MR12345678
- `patent-pro`: 专利号 CN20231012345678.X
- `presales-pro`: 内部客户简称（项目代号）

## 关键设计：可逆 placeholder

```
原文：   13812345678 致电 user@example.com 询问劳动合同
           ↓ Redactor.redact()
脱敏：   [PHONE_1] 致电 [EMAIL_1] 询问劳动合同
           ↓ 云端 LLM 处理
LLM 答案：根据 [EMAIL_1] 的咨询，[PHONE_1] 应当...
           ↓ Redactor.restore()
还原：   根据 user@example.com 的咨询，13812345678 应当...
```

**特性**：
- 同值同 placeholder（"张三"出现 3 次都得 `[PERSON_1]`）— LLM 语义一致
- 字典+正则双引擎（用户自定义 `.attune/pii_dict.yaml`）
- vertical plugin 通过 `plugin.yaml::pii_patterns` 注入行业 PII

## 出网审计日志

每次云 LLM 调用都本地落 audit log（**0 用户原文落库**）：

```sql
CREATE TABLE outbound_audit (
    id               INTEGER PRIMARY KEY,
    ts_ms            INTEGER NOT NULL,
    direction        TEXT NOT NULL,      -- 'request' | 'response'
    provider         TEXT NOT NULL,      -- 'anthropic' / 'openai' / 'ollama'
    model            TEXT NOT NULL,
    token_estimate   INTEGER,
    privacy_tier     TEXT NOT NULL,      -- 'L0' / 'L1' / 'L3'
    pre_redact_hash  TEXT NOT NULL,      -- SHA256[:16] 脱敏前
    post_redact_hash TEXT NOT NULL,      -- SHA256[:16] 脱敏后
    redactions_json  TEXT NOT NULL,      -- {"PHONE":2,"EMAIL":1,"CASE_NO":3}
    session_id       TEXT
);
```

**用法**（合规员典型工作流）：
```
Settings → Privacy → 出网审计 → 导出 CSV (任意时段)
→ 给法务 / 审计员 / 数据保护官
```

## per-file 🔒 标记 (L0)

任何 item 可被标记为 `privacy_tier='L0'`，chunk 在 chat retrieval 阶段会被
`Store::filter_out_l0_items()` 过滤掉。

API:
```bash
# 标记某文件为 L0
curl -X PATCH http://localhost:18900/api/v1/items/{id}/privacy_tier \
  -H 'Authorization: Bearer $TOKEN' \
  -H 'Content-Type: application/json' \
  -d '{"tier":"L0"}'

# 列出所有 L0 文件
curl http://localhost:18900/api/v1/items/protected
```

UI（v0.6.1 上线）：文件列表右键 → "标记为 🔒 机密"。

## 当前可量化指标

| 指标 | 当前 | v0.7 目标 |
|------|------|----------|
| `pii_leak_rate` (格式化) | ≤ 0.5% (实测在 100 合同) | ≤ 0.1% |
| `pii_leak_rate` (语义) | ~22% (无 L3) | ≤ 5% (L3 启用后) |
| `restoration_accuracy` | ≥ 99.5% | 同 |
| `audit_completeness` | 100% (强制) | 同 |

## 与 1Password / Bitwarden 的区别

| | 1Password | Bitwarden | **Attune** |
|---|---|---|---|
| 存储模型 | 加密同步到云 | 加密同步到云 | **本地 vault**，可选导出 |
| 云端可见 | 加密 blob | 加密 blob | **完全不可见原文**，只见脱敏片段 |
| 主用例 | 密码 | 密码 + 2FA | **私有知识库 + RAG** |
| 证据流 | 不适用 | 不适用 | **chunk-level breadcrumb + offset** |

## 路线图

- ✅ **v0.6.0** — L1 完整、L0 per-file、出网审计、F-Pro 跨域防御
- 🟡 **v0.7** — L3 LLM 语义脱敏（chinese-roberta NER + Tier T3+/K3 自动启用）
- 🟡 **v0.7** — Settings → Privacy 完整 UI（per-folder override + tier 升级提示）
- 🟡 **v0.8** — K3 一体机 L0 全本地链路（embedding/rerank/LLM 全 K3 服务，0 公网）
