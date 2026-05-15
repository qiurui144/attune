# Plugin 开发指南

attune plugin 系统让第三方/行业方扩展 attune 能力. 当前 4 个 attune-pro vertical
(law / patent / sales / tech) 使用此机制. 本文档面向 plugin 开发者.

## Plugin 形态

每个 plugin 是一个目录或 .attpkg 压缩包, 包含:

```
my-plugin/
├── plugin.yaml          # 元信息 + entities + keywords + skills + workflows
├── prompt.md            # (可选) 自定义 system prompt 片段
├── README.md            # (可选) 用户面向说明
├── icon.png             # 16×16/48×48 png
└── forms/               # (可选) 自定义表单
    └── intake.yaml
```

## plugin.yaml schema

```yaml
# 必填
id: law-pro                    # kebab-case, attune-pro 仓 vertical-pro 命名
version: "0.6.0"               # semver
name: "法律 Pro"
description: "律师专属知识库增强 (案号识别 / 法条匹配 / 卷宗结构)"

# 分类 (用于 marketplace UI)
category: vertical             # vertical / utility / theme
vertical: law                  # 如 category=vertical

# 触发关键词 (chat 中检测到这些词倾向用本 plugin 的 prompt + skill)
trigger_keywords:
  - "起诉"
  - "判决书"
  - "合同条款"

# Entity 提取规则 (attune-core::entities 会注册)
entities:
  - kind: case_no              # 自定义 EntityKind
    pattern: '\(?\d{4}\)\?\s*[一-龥]\d+号'  # 中文案号
    description: "中文法律案号 ISO XX"

# Skills (注册到 SkillRegistry)
skills:
  - id: parse_case_doc
    description: "解析判决书结构"
    impl: builtin/parse_case_doc  # 或 rpa: filename, llm: file

# Workflows (n8n-style chained step)
workflows:
  - id: case_intake
    name: "案件归档流程"
    steps:
      - skill: parse_case_doc
      - skill: extract_parties
      - skill: classify_case_type

# (可选) Project / Case 卷宗 schema
project_schema:
  fields:
    - name: 当事人
      type: string
    - name: 立案日期
      type: date
```

## Signing (CI 用)

attune 强制 official-key 签名 (防 supply chain). 流程:

```bash
# 1. 生成 Ed25519 私钥 (一次性, 离线安全存)
attune-cli plugin keygen > my-key.priv

# 2. 公钥 hex 嵌入 OFFICIAL_PUBLIC_KEYS (attune-core::plugin_sig)
attune-cli plugin pubkey-hex my-key.priv

# 3. 打包 + 签名
attune-cli plugin pack my-plugin/ --sign my-key.priv -o my-plugin-0.6.0.attpkg
```

`.attpkg` 是 zip 含 plugin 目录 + `signature.bin` (Ed25519 over content hash).

## Encryption (商业 plugin)

付费 plugin 可加密 plugin.yaml + skills 实现:

```bash
attune-cli plugin encrypt my-plugin.attpkg \
  --password <license-key> \
  -o my-plugin.attpkg.enc
```

attune 装载时用 device-bound license key 解密 (per `plugin_encryption.rs`,
Argon2id + AES-GCM, OsRng nonce + salt).

## Install flow (用户视角)

1. 用户在 marketplace UI 看到 plugin 卡 (来自 `pluginhub.url` 或本地 plugins/)
2. 点 "安装" → POST /api/v1/marketplace/plugins/{id}/install
3. attune-server 调 pluginhub HTTP 下载 .attpkg
4. plugin_sig::verify 签名校验
5. (付费) plugin_encryption::decrypt_yaml 解密
6. 写入 ~/.local/share/attune/plugins/{id}/
7. state.plugin_registry.reload() 热载
8. taxonomy / entities / skills / workflows 注册到 in-memory hub
9. UI 实时显示 (无需重启)

## 4 attune-pro vertical 示例

attune-pro 仓 (闭源) 已实施:

| Plugin | trigger | 主 skill |
|--------|---------|----------|
| law-pro | "起诉" "判决书" | parse_case_doc / extract_parties / classify_case_type |
| patent-pro | "专利" "权利要求" | parse_claim / classify_ipc / patent_diff |
| sales-pro | "客户" "报价" | extract_bant / generate_quote / objection_handle |
| tech-pro | "项目" "需求" | parse_prd / extract_stories / estimate_effort |

每个 plugin 在 OSS attune 内有"试用卡片"显示 (mock provider), 装 attune-pro
membership 后真功能解锁.

## API endpoints

- `GET /api/v1/plugins` — 已装 plugin 列表 (per state.plugin_registry)
- `GET /api/v1/marketplace/plugins` — 远端 hub 可装 plugin 列表
- `POST /api/v1/marketplace/plugins/{id}/install` — 装 plugin
- `DELETE /api/v1/plugins/{id}` — 卸载

## 开发本地测试

```bash
# 1. 链接本地 plugin 到 attune 数据目录
ln -s $PWD/my-plugin ~/.local/share/attune/plugins/my-plugin

# 2. 启动 attune-server
attune-server-headless

# 3. 触发 reload
curl -X POST http://localhost:18900/api/v1/plugins/reload
```

详细 plugin signing / encryption 实现见:
- `crates/attune-core/src/plugin_sig.rs` — Ed25519 签名
- `crates/attune-core/src/plugin_encryption.rs` — Argon2id+AES-GCM 加密
- `crates/attune-core/src/plugin_loader.rs` — yaml 解析 + 装载
- `crates/attune-core/src/plugin_registry.rs` — registry + match + dispatch
- `crates/attune-core/src/plugin_hub.rs` — remote hub provider trait
