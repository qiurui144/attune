# Attune 架构

## 双产品线

```
┌─────────────────────────────┐  ┌─────────────────────────────┐
│  Python 原型线 (实验)         │  │  Rust 商用线 (生产)          │
│  src/npu_webhook/           │  │  rust/                       │
│  • FastAPI + ChromaDB       │  │  • Axum + rusqlite           │
│  • 78 tests                 │  │  • 900+ tests                │
│  • 快速迭代新 feature       │  │  • AES-256-GCM 加密 vault    │
└─────────────────────────────┘  └─────────────────────────────┘
                ↓ 验证后择优迁移
                选 Rust 商用线 ship
```

设计取舍：Python 端验证算法 / 集成 / UX；Rust 端打包发版 / 加密 / 性能。

## Rust 后端模块（v0.7）

```
attune-core/
├── chat/              # ChatEngine（J5 Self-RAG + 二次检索）
├── chunker/           # Markdown 分块 + 章节路径 breadcrumb
├── context_budget/    # ★ v0.7 LLM 上下文窗口预算（注入/历史按模型窗口分配）
├── classifier/        # 维度分类（按 plugin 注册）
├── clusterer/         # HDBSCAN 聚类
├── crypto/            # Argon2id + AES-GCM
├── embed/             # OllamaProvider + OrtEmbeddingProvider (Xenova/bge-m3 量化)
├── entities/          # 通用实体抽取（Person/Money/Date/Org）
├── infer/
│   ├── embedding.rs   # ONNX bge-m3 推理
│   ├── reranker.rs    # ONNX BAAI/bge-reranker-base 推理
│   └── model_store.rs # HF 模型自动下载 + 缓存
├── pii/               # ★ Phase A.5
│   ├── mod.rs         # Redactor + PiiKind + PiiMatch
│   ├── patterns.rs    # 12 类格式化 PII（含 ISO 7064 / Luhn）
│   ├── dictionary.rs  # 用户自定义词典（YAML）
│   └── ner.rs         # L2 NER trait scaffold (v0.7 ONNX)
├── platform/          # CPU 性能 DB + Tier 分类 + Region 检测
├── plugin_loader/     # plugin.yaml 解析（含 chat_trigger / pii_patterns）
├── plugin_registry/   # 插件全局聚合（chat keywords + PII patterns）
├── search/            # 三阶段：BM25 + 向量 → RRF → reranker → cross-domain penalty
├── store/             # rusqlite 持久层
│   ├── items          # 主资产表（含 privacy_tier + corpus_domain）
│   ├── item_blobs     # ★ v0.7 原始上传文件加密留存（证据原文可回看）
│   ├── audit          # ★ 出网审计日志（明文 metadata）
│   ├── chunk_breadcrumbs  # F2 加密 sidecar
│   └── ...
├── vault/             # 主控 + DEK 派生
└── workflow/          # 三段式（extract_entities → find_overlap → write_annotation）

attune-server/
├── routes/
│   ├── chat.rs        # /api/v1/chat（J5 + 证据流）
│   ├── search.rs      # /api/v1/search（含 detect_query_domain）
│   ├── audit.rs       # ★ 出网审计 + CSV 导出
│   ├── privacy.rs     # ★ Privacy tier endpoint
│   ├── items.rs       # ★ per-file privacy_tier set/get
│   └── ...
└── ui/                # 嵌入式 Preact + Tailwind 单页（include_str!）

attune-cli/            # 命令行工具
attune-tauri/          # 桌面应用（Tauri 2 + Preact UI）
```

## 检索栈（Phase B 验证）

```
                User query
                    ↓
       ┌─────────────────────────┐
       │  detect_query_domain    │  关键词 4 domain × 12-30 词，零 LLM
       │  → "legal"/"tech"/...   │
       └──────────────┬──────────┘
                      ↓
       ┌──────────┐  ┌──────────┐
       │  BM25    │  │  Vector  │  双路检索
       │ tantivy  │  │  usearch │  
       │ +jieba   │  │ +bge-m3  │
       └────┬─────┘  └────┬─────┘
            ↓             ↓
            └──────┬──────┘
                   ↓
              RRF 融合 (top-20)
                   ↓
         ┌─────────────────┐
         │ BAAI/bge-       │  Cross-encoder 真值排序
         │  reranker-base  │  ONNX (Xenova 量化版触发 Expand bug，已切官方)
         └─────────┬───────┘
                   ↓
       Cross-language penalty   query.lang ≠ doc.lang → ×0.3
                   ↓
       Cross-domain  penalty    query.domain ≠ doc.domain → ×0.4  ★ F-Pro
                   ↓
              Final top-K
```

## 隐私架构（Phase A.5）

```
                User upload file
                       ↓
              本地解析 + 分块
                       ↓
              embed_queue 入队
                       ↓
       ┌─────────── 全程 100% 本地 ───────────┐
       │  bge-m3 embedding → vector index     │
       │  jieba tokenize  → tantivy fts5      │
       │  AES-256-GCM 加密 → vault.db          │
       └───────────────┬───────────────────────┘
                       ↓
                  User query
                       ↓
              检索 top-5 chunks
                       ↓
       ┌──────────────┴──────────────┐
       │  per-file 🔒 (L0)?          │
       │  → 强制本地 LLM             │
       └──────────────┬──────────────┘
                       ↓ (L1 默认)
       ┌──────────────────────────────┐
       │  PII Redactor (本地)         │
       │  • 12 类格式化 PII 检测       │
       │  • 替换为 [PERSON_1] 等      │
       │  • 同值同 placeholder        │
       └──────────────┬───────────────┘
                       ↓
       ┌──────────────────────────────┐
       │  出网审计日志写入             │
       │  • SHA256[:16] 前后 hash     │
       │  • 模型 / token / 脱敏统计   │
       │  • 0 用户原文落库             │
       └──────────────┬───────────────┘
                       ↓
              ☁️  云端 LLM（脱敏后）
                       ↓
              本地 PII 还原（placeholder → 原值）
                       ↓
                User 看到完整答案 + citations
```

## 资源治理（H1）

每个后台任务（embedding worker / classify / browse capture）通过 `resource_governor`
注册：
- per-task CPU budget
- 全局 RAM ceiling
- 暂停/恢复 hook

避免长会话把 GPU/RAM 跑飞。

## 浏览捕获（W3 batch B）

```
Chrome 扩展 → content_browse_capture.js
   ↓ MutationObserver + dwell timer
浏览信号（URL hash + dwell + scroll + copy）
   ↓ 加密上传
attune-server browse_signals 表
   ↓ G2 high engagement 阈值（dwell ≥3min + scroll ≥50% + copy ≥1）
auto_bookmark candidate
   ↓ G3 worker (W5+)
正文抓取 + promote 到 items
   ↓
RAG 知识库
```

## Reliability Framework（v1.0）

agent 可靠性通过三 Phase gate 强制保证，每个 law-pro agent 并入 develop 前必须全过：

```
                 agent PR
                     ↓
        ┌─────────────────────────┐
        │  Phase 1: F1 gate       │  确定性 agent 公式 / 规则输出 F1 = 1.00
        │  (golden fixtures CI)   │
        └──────────────┬──────────┘
                       ↓ pass
        ┌─────────────────────────┐
        │  Phase 2: Six-class     │  6 类下限 ENFORCE（structural gate）
        │  floor gate (CI)        │  覆盖所有 agent 输出类型，不低于最小集
        └──────────────┬──────────┘
                       ↓ pass
        ┌─────────────────────────┐
        │  Phase 3: LLM gate      │  LLM extractor 语义 F1 ≥ 0.85
        │  (llm-holdout eval)     │  v1.0 实测 0.9828（10 holdout cases）
        └──────────────┬──────────┘
                       ↓ all pass
                  merge to develop
```

| Phase | 触发时机 | v1.0 实测 |
|-------|---------|----------|
| Phase 1 | 每次 PR CI | F1 = 1.00 |
| Phase 2 | 每次 PR CI | all pass |
| Phase 3 | LLM holdout eval（手动触发） | F1 = 0.9828 |

## 模型矩阵（按硬件 tier）

| Tier | RAM | embedding | reranker | LLM |
|------|-----|-----------|----------|-----|
| Low | 4-8GB | bge-small (ORT) | base | qwen2.5:1.5b |
| Mid | 8-16GB | bge-base (ORT) | base | qwen2.5:3b |
| **High** | **16-32GB** | **bge-m3 (Ollama F16)** | **bge-reranker-base** | **deepseek-r1:14b** |
| Flagship | 32+GB | bge-m3 + 自训 fine-tune | v2-m3 multilingual | qwen3.5:35b-a3b |

`ATTUNE_EMBEDDING_BACKEND=ollama` / `ATTUNE_CHAT_MODEL=<name>` 可手动覆盖。
