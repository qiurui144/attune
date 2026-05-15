# attune v0.7 功能缺口分析

**Date**: 2026-05-15  
**基线**: v0.6.3 GA (commit ec95619) + D-R 30 轮 sprint (commit 6e4eb96) + screenshot 整理 (commit 32350df)

按 14 个产品维度评估当前 (v0.6.3) vs 目标. 缺口按 RICE 排序, 标注是否 OSS attune / attune-pro 范围.

## 0. 决策矩阵 (本 doc 落 v0.7 路线图)

| Layer | 当前状态 | v0.7 目标 |
|-------|---------|---------|
| Capture (输入) | 4 渠道 (folder / upload / Chrome ext / browse signal) | +2 (Telegram bot / RSS) |
| Process (索引) | 5 format + OCR | +VLM 图理解 (Tier T2+) |
| Retrieve (检索) | RRF + cross-domain + cite | +query rewriting + 时间检索 |
| Reason (chat) | RAG + PII + compress | +tool calling 通用 + streaming UI |
| Annotate | user + ai source | +Reader PDF highlight |
| Project | schema + chat_trigger | +Pro export 报告 |
| Skill | yaml + workflow | +SkillClaw 评估 framework |
| Sync | manual export/import | +定时云备份 (BYOK S3/WebDAV) |
| Collab | by design 不做 | (no change) |
| Visualizations | Knowledge Map (cluster) | +timeline view |
| Privacy | 3 tier + 12 PII + audit (占位) | audit 真持久化 + CSV export |
| Onboarding | 5-step wizard | +Demo sample data 入口 |
| Cost UI | CLAUDE.md 契约 | UI chip 实际显示 token est |
| Themes | dark token | +字号 / 多主题 |

---

## 1. Capture (入库渠道)

**已有**:
- ✅ 本地文件夹监听 (watchdog → indexer.pipeline)
- ✅ 拖拽 / API upload (parser parse_bytes)
- ✅ Chrome 扩展捕获 ChatGPT / Claude / Gemini 对话 (MV3)
- ✅ G1 浏览状态信号 (browse_capture all_urls 排除 login/AI/密码管理)
- ✅ WebDAV 远程目录扫描 (scanner_webdav.rs)
- ✅ 专利数据库扫描 (scanner_patent.rs, attune-pro 行业)

**缺口**:
| 项 | 用户 reach | 价值 | OSS/Pro | RICE |
|---|------|------|---------|------|
| Email 入库 (IMAP / Gmail API) | 高 | 高 | OSS | 高 |
| Telegram bot / Channel 监听 | 中 | 高 | OSS | 中 |
| RSS / Atom 订阅 | 中 | 中 | OSS | 中 |
| 微信 / 飞书 IM (国内主流) | 高 | 高 | attune-pro 中文版 | 高 (合规复杂) |
| API webhook (e.g. Notion API push) | 低 | 中 | OSS | 低 |

**v0.7 建议**: 优先 Email IMAP (rich content, user 普遍有). Telegram + RSS 跟上.

## 2. Process (索引 + 分类)

**已有**:
- ✅ 5 format parser (md/pdf/docx/code/text)
- ✅ asr.rs whisper-cli + diarization
- ✅ OCR PP-OCRv5 mobile 21 MB
- ✅ chunker 滑窗 + code fence balance + 章节切割
- ✅ classifier Ollama + plugin taxonomy
- ✅ clusterer hdbscan

**缺口**:
| 项 | RICE |
|---|------|
| VLM 图像理解 (caption / scene description, 不仅 OCR 文字) | 高 (Tier T2+, llava / qwen-vl) |
| 视频内容理解 (key frame + caption, 不仅 ASR) | 中 (大型, v0.8) |
| 表格语义化 (Excel/PDF table → 结构化 KV pairs) | 中 |
| 公式 LaTeX 识别 (科研场景) | 低 |

**v0.7 建议**: VLM 图理解优先 (llava + ollama 已成熟生态), 大幅扩展图像类知识库价值.

## 3. Retrieve (检索)

**已有**:
- ✅ RRF 混合 (vector + FTS5)
- ✅ Cross-language + cross-domain penalty (F-Pro)
- ✅ 两阶段层级 (Level 1 章节 → Level 2 段落)
- ✅ Cite chain (breadcrumb + chunk_offset_start/end)
- ✅ Reranker bge-reranker-v2-m3

**缺口**:
| 项 | RICE |
|---|------|
| Query rewriting (LLM 把用户口语 query 改写为关键词) | 高 (大幅提升 hit rate) |
| 时间旅行检索 (when-based: "上周谁说了 X") | 高 (chat history 与 item 时间维度) |
| Entity graph (人 / 项目 / 时间 / 地点关联图) | 中 (cluster 是表象, graph 是结构) |
| 同义词 / 别名扩展 (例 "GPT-4" → "ChatGPT 4") | 中 |
| HNSW params expose to settings (D-R4) | 低 (advanced) |

**v0.7 建议**: Query rewriting (1 day 实施, RICE 最高 wave).

## 4. Reason (chat / RAG)

**已有**:
- ✅ ChatEngine + PII redact 全路径 (F-17)
- ✅ Citation extraction (LLM strict-prompt marker)
- ✅ Context compress (budget aware + cite preserve)
- ✅ 多 LLM provider (OpenAI / Anthropic / Gemini / DeepSeek / Qwen / custom / Ollama)

**缺口**:
| 项 | RICE | 备注 |
|---|------|------|
| Tool calling generic (非仅 web search, 也 fs / shell / plugin skill) | 高 | Anthropic / OpenAI function-call 通用 schema |
| Streaming output UI | 高 | CLAUDE.md 写"v0.6 不做", v0.7 SSE 接通 (decision pending) |
| Multi-turn reasoning UI (chain-of-thought 可视化) | 中 | Claude 4.7 thinking blocks 支持 |
| Multi-agent (e.g. researcher → critic → writer) | 低 | Pro/复杂场景, v0.8+ |

**v0.7 建议**: Tool calling generic + streaming UI (per user 体感最强).

## 5. Annotate (人工标注)

**已有**:
- ✅ annotations API (user / ai source)
- ✅ ai_annotator (chunks 自动加 ai annotation)
- ✅ annotation_weight (in search ranking)

**缺口**:
| 项 | RICE |
|---|------|
| **Reader 模式 PDF 内嵌 highlight + 划词标注** | 高 (CLAUDE.md 提到 Reader 模态待 E2E) |
| Web 页面长按选词加标注 (Chrome 扩展) | 中 |
| Voice annotation (whisper inline) | 低 |

**v0.7 建议**: Reader PDF highlight 是产品差异化关键 (vs 一般 RAG).

## 6. Project / Case 卷宗

**已有**:
- ✅ Project schema + metadata_encrypted
- ✅ project_recommender chat_trigger keywords
- ✅ Case 卷宗 (attune-pro 4 vertical)

**缺口**:
| 项 | RICE |
|---|------|
| Project 报告自动生成 (基于 cite chain LLM 整合) | 高 (Pro 商业化关键) |
| Project tree view UI (层级卷宗) | 中 |
| 跨 Project 跳转 + 引用 | 低 |

**v0.7 建议**: 报告生成 (Pro 收入路径).

## 7. Skill / Automation

**已有**:
- ✅ Skills (yaml + builtin impl)
- ✅ Workflows (chained steps)
- ✅ SkillClaw 风格 skill_evolution (后台静默扩展词, 见 chat.rs)

**缺口**:
| 项 | RICE |
|---|------|
| Skill evaluation framework (acc / latency / cost 评分 + 自动 A/B) | 中 (per CLAUDE.md SkillClaw) |
| RPA 录制 (类 lawcontrol RPA) | 中 (Pro / 行业) |
| Skill marketplace 评分 / 推荐 (per R20 P2) | 中 |

## 8. Sync / Backup

**已有**:
- ✅ .vault-profile export / import (手动 wizard step 5)
- ✅ device_secret 双设备绑定 (设备主迁移)

**缺口**:
| 项 | RICE |
|---|------|
| 定时自动云备份 (BYOK S3 / WebDAV / GDrive) | 高 (用户痛点) |
| 增量备份 (只传 diff, 不是全量 .vault-profile) | 中 |
| 移动 app | by design 暂不做 (per CLAUDE.md M2) |

**v0.7 建议**: WebDAV 定时备份 (Nextcloud / Dropbox 都兼容).

## 9. Collaboration

**By design 不做** (per CLAUDE.md "数据完全隔离, 不与任何外部产品同步"). lawcontrol 是 B2B 协作版.

## 10. Visualization

**已有**:
- ✅ Knowledge Map (knowledge 全景 tab, hdbscan cluster 2D 可视)

**缺口**:
| 项 | RICE |
|---|------|
| Timeline view (按时间 axis 显示 items / chat) | 中 |
| 标签云 (tag frequency) | 低 |
| Entity relation graph (人 / 项目 节点边) | 中 (D-R3 提) |

## 11. Privacy / Security

**已有 (D-R27 验证)**:
- ✅ Argon2id + AES-256-GCM 字段级
- ✅ Device Secret 派生
- ✅ 3 tier 隐私 (L0 永不出网 / L1 12 PII 脱敏 / L3 LLM 语义脱敏 v0.7)
- ✅ TLS rustls

**缺口** (D-R11 提):
| 项 | RICE |
|---|------|
| **audit_log 真持久化 + CSV export** | 高 (合规 + UI 入口已 wire, 后端补 0.5 day) |
| Vault 完整性 hash 检测 (防离线篡改) | 中 |
| L3 LLM 语义脱敏 (Tier T3+ / K3 硬件) | 中 (per Phase A.5.3 spec) |

**v0.7 建议**: audit_log 真持久化是承诺已 ship 一半, 必须 close loop.

## 12. Onboarding / Discovery

**已有 (v063 截图验证)**:
- ✅ 5-step wizard (welcome / password / AI / hardware / data)
- ✅ FormFactor 自动检测 LLM 默认

**缺口**:
| 项 | RICE |
|---|------|
| Demo / sample data 加载 (新用户 "先看看") | 高 (留存) |
| In-app tour / tooltip (首启引导) | 中 |
| 文档 / FAQ 内嵌 | 中 |

**v0.7 建议**: sample dataset (e.g. 100 篇 Wikipedia / GitHub README) wizard 选项, 大幅降低"空知识库无意义"流失.

## 13. Cost Awareness (UI 落实)

**已有 (per CLAUDE.md 三层成本契约)**:
- ✅ 零成本 / 本地算力 / 时间金钱 三层 doc
- ✅ 后台任务可暂停开关

**缺口** (Cost & Trigger Contract 实际 UI):
| 项 | RICE |
|---|------|
| Chat 发送按钮旁 token 估算 chip (`~1.2K tok · $0.0004`) | 高 (CLAUDE.md 明示) |
| 每个 AI 分析按钮标本地/云端 + 预估耗时 | 中 |
| 顶栏后台任务队列可见 + 暂停 | 中 |

## 14. Themes / Customization

**已有**:
- ✅ Dark mode token (settings-darkmode 截图)
- ✅ Locale (zh / en 切换)

**缺口**:
| 项 | RICE |
|---|------|
| 字号 / 字体大小 (老花眼用户) | 中 |
| 多主题 / 色盲友好 | 低 |
| Layout 切换 (compact / cozy) | 低 |

---

## 综合 v0.7 P0 提名 (按 RICE)

按"reach × impact × confidence / effort" 综合:

| Rank | 项 | RICE | Effort | OSS/Pro |
|------|---|------|-------|---------|
| 1 | Email 入库 (IMAP / Gmail) | 极高 | 2 day | OSS |
| 2 | Query rewriting (LLM 改写) | 极高 | 1 day | OSS |
| 3 | Audit log 真持久化 (close F-17 loop) | 高 | 0.5 day | OSS |
| 4 | Cost UI chip (token est) | 高 | 0.5 day | OSS |
| 5 | Tool calling generic | 高 | 2 day | OSS |
| 6 | Streaming chat output | 高 | 1 day | OSS |
| 7 | Demo sample data | 高 | 0.5 day | OSS |
| 8 | Reader PDF highlight | 高 | 3 day | OSS |
| 9 | VLM 图理解 | 中 | 2 day | OSS |
| 10 | Project 报告生成 | 中 | 2 day | attune-pro |
| 11 | WebDAV 定时备份 | 中 | 1 day | OSS |
| 12 | Time travel search | 中 | 1 day | OSS |
| 13 | Entity graph | 中 | 2 day | OSS |
| 14 | Skill evaluation framework | 中 | 2 day | OSS |
| 15 | Telegram bot 入库 | 中 | 1 day | OSS |

**v0.7 sprint 建议** (单 sprint 1 week budget = ~10 day):

**Track A (RAG quality)**: Query rewriting + Tool calling + Streaming UI + Cost chip = 4 day
**Track B (capture/onboarding)**: Email + Demo sample data + Telegram = 3.5 day
**Track C (debt cleanup)**: Audit log persist + ArcSwap real migration (D-R14) + 37 routes AppError (D-R13) = 3.5 day

合计 11 day, 留 1 day buffer.

**v0.8+ 路线**: Reader highlight / VLM / WebDAV / Project 报告生成 / Entity graph.
