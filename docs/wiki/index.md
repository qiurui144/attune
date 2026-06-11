# Attune — 私有 AI 知识伙伴

> 你的本地知识 + 你能控制的 AI = Attune。  
> 桌面应用 + 浏览器扩展，一切数据先在本地落地，云 LLM 只看脱敏后的片段。

## 这是什么

Attune 是一个**私有知识库 + 记忆增强系统**：
- 把你的笔记、文档、网页阅读痕迹，统一索引成可检索可追溯的本地知识库
- 在 Chat / RAG 时只让云 LLM 看脱敏后的相关片段，**原文 100% 不出网**
- 检索栈基于业界 SOTA：bge-m3 双语 embedding + BAAI bge-reranker + cross-domain penalty
- 三赛道 PRO 级 benchmark 验证（法律 / 通用英文 / 中文八股）

## 三产品矩阵

| 产品 | 形态 | 用户群 | License |
|------|------|--------|---------|
| **Attune (OSS)** | 桌面 / Chrome 扩展 | 个人通用用户 | Apache-2.0 |
| **Attune Pro** | Plugin pack 装载到 Attune | 个人行业用户（律师/医生/学者/售前/工程师/专利代理）| 商业（订阅）|
| **Attune Enterprise** | Django + Vue + 19 容器 SaaS | 律所 / 小团队 B2B | 商业（License）|

**等式**：
- 个人通用用户 = Attune (OSS)
- 个人行业用户 = Attune (OSS) + Attune-Pro/<vertical>-pro
- 行业小团队 = Attune Enterprise

## v1.0 亮点

- **🤖 20 个 AI agent 全面投产**
  - law-pro：11 个确定性 agent（民事借贷 / 劳动争议 / 诉讼时效 / 证据链 / 名誉权 / 法定继承 等）+ 3 个 LLM extractor
  - OSS 内置：4 个 AI 批注 agent（highlights / questions / risk / outdated） + document classifier
  - Office helper：OCR + ASR transcription（PP-OCRv5 mobile + whisper.cpp）

- **🛡️ Reliability framework — 3 Phase gate**
  - Phase 1: 确定性 agent 输出 gate（F1 = 1.00）
  - Phase 2: 6 类下限 ENFORCE（structural gate，CI 强制）
  - Phase 3: LLM gate F1 ≥ 0.85（实测 0.9828）

- **🎯 三赛道全 PRO 级 benchmark**（2026-04-28 实测，v1.0 保持）
  - 法律: Hit@10=0.80, MRR=0.50
  - Rust/英文: Hit@10=1.00, MRR=1.00 ⭐ 满分
  - 中文八股: Hit@10=1.00, MRR=1.00 ⭐ 满分
  - law-pro golden_qa 5 维度: **25/25 满分** (vs legal baseline +39%)

- **📥 5 个采集源内置 connector**
  - Local / Email (IMAP) / WebDAV / RSS / Telegram scaffold

- **🔒 三层隐私模型 (Phase A.5)**
  - L0: 文件级 🔒，永不出网（强制本地 LLM）
  - L1: 12 类格式化 PII 自动脱敏 + 出网审计 + CSV 导出（默认）
  - L3: LLM 语义脱敏（K3 一体机 / 高端硬件）

- **🌐 跨域污染防御 (F-Pro)**
  - corpus_domain 字段 + 领域前缀注入 + cross-domain penalty
  - 关键词 query intent 检测（零 LLM 调用，亚毫秒）

- **📋 证据流端到端**
  - chat citation 含 breadcrumb（章节路径）+ chunk_offset（Reader 跳转锚点）
  - confidence 1-5 自评（J5 Self-RAG 框架）

## 快速入口

<div class="product-grid">
  <a class="product-card" href="quickstart/">
    <span style="font-size:2rem">🚀</span>
    <h3>快速开始</h3>
    <p>5 分钟安装 + 第一次问答</p>
  </a>
  <a class="product-card" href="agents/">
    <span style="font-size:2rem">🤖</span>
    <h3>Agent 详介</h3>
    <p>20 个 agent：law-pro 14 + OSS 4 + Office 2</p>
  </a>
  <a class="product-card" href="architecture/">
    <span style="font-size:2rem">🏗️</span>
    <h3>架构</h3>
    <p>双产品线（Python 原型 + Rust 商用）+ 检索栈解析</p>
  </a>
  <a class="product-card" href="privacy/">
    <span style="font-size:2rem">🔒</span>
    <h3>隐私模型</h3>
    <p>L0/L1/L3 三层脱敏 + 出网审计 + per-file 🔒</p>
  </a>
  <a class="product-card" href="benchmarks/">
    <span style="font-size:2rem">📊</span>
    <h3>Benchmark 数字</h3>
    <p>三赛道 + 5 维度评分 + Reliability F1</p>
  </a>
</div>

## 反差优势（对比通用 AI 客户端）

| 痛点 | Attune 怎么解决 |
|------|---------------|
| 把整个文件传给 ChatGPT 后无法撤回 | **原文从不出网**，只发 ≤3000 字脱敏片段 |
| 答案声称"在文档里"但找不到出处 | **每条引用都是真链接**：title + 章节路径 + chunk offset |
| 按月烧 token 但 70% query 是重复的 | **本地命中率 ≥ 70%**（embed/rerank 全本地，LLM 只做 final synthesis）|
| 中文法律问答总混入技术内容 | **F-Pro 跨域防御**：query "反洗钱" 不拉 Java 算法 |
| 上传文档后无法证明"删除了"| **all-encrypted by DEK + audit log + per-file 🔒**（合规可审计）|

## 资源

- 源代码：[github.com/qiurui144/attune](https://github.com/qiurui144/attune)
- 下载 v1.0.0：[GitHub Releases](https://github.com/qiurui144/attune/releases/tag/v1.0.0) (Linux deb/AppImage + Windows MSI)
- 中文 README：[README.zh-CN.md](https://github.com/qiurui144/attune/blob/develop/README.zh-CN.md)
- Benchmark 详情：[docs/benchmarks/](https://github.com/qiurui144/attune/blob/develop/docs/benchmarks/)
- Reliability framework：[docs/superpowers/specs/](https://github.com/qiurui144/attune/blob/develop/docs/superpowers/specs/)

## License

- **Attune (OSS)**: Apache-2.0
- **Attune Pro**: Proprietary plugin pack（订阅制，详见 [价格 & 计划](/plans/attune-pricing)）
