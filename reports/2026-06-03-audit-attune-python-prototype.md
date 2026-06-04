# Audit: attune-python-prototype (Python 原型线)

- **Date**: 2026-06-03
- **Area**: `/data/company/project/attune/python` — FastAPI + ChromaDB + SQLite FTS5 prototype line
- **Scope note**: 任务 prompt 标 "71483 LOC"，实测该数字含 `.venv` 第三方依赖（`find . -name '*.py' | wc -l` = 5336 文件 / 394540 LOC 全含 venv）。**真正第一方手写代码 ≈ 6354 LOC**:`src/attune_python/` 3971 LOC + `tests/` `tests-e2e/` 2383 LOC。审计聚焦该 6.4K。
- **Method**: Glob/Grep 摸结构 + 读全部核心模块(detector / sqlite_db / search / embedding / main / pipeline / queue / chunker / 全 api/*) + grep 死代码/schema 漂移/版本。

---

## Scorecard

| 维度 | 分(1差5优) | 说明 |
|------|:--:|------|
| code_quality | **4** | 单文件干净、类型注解齐、错误处理细(原子 UPDATE 防竞态 / fail-closed auth / N+1 已批量化)。扣分:多处函数内 `import`、stub 路由混在生产路由表。 |
| complexity | **4** | 最大文件 719 行(detector.py,纯数据表+线性匹配,非真热点)。无深嵌套巨函数。整体复杂度低。 |
| simplification_potential | **2** | 原型线与 Rust 商用线**功能高度重叠**(SQLite/FTS/向量/RRF/embedding/平台检测全部 Rust 已实装)。按 README 自身定位"验证有效特性已 promote 到 Rust",大量模块可视为**已毕业可冻结/精简**;另有真死代码 + schema 漂移 ~250-400 LOC 可直接删。 |
| doc_accuracy | **2** | 多处 doc-drift:CLAUDE.md/README 宣称的"两层层级索引(章节 Level1+段落 Level2)"、`extract_sections`、`search_relevant 两阶段层级检索`、"73 tests"/"13 tests" 在代码中**不存在或不一致**。 |

---

## 分维度 Findings

### (1) 正确性 / silent failure

- **[low] `core/search.py:95`** `Reranker._embed`:`self._cache.put(key, [emb])` 存为 `[emb]`(嵌套 list),但命中分支 `results[i] = cached[0]` 取 `[0]` ——自洽但绕。真正风险:若 `api_result["embeddings"]` 长度 < `uncached_idx` 长度(Ollama 部分失败),`api_result["embeddings"][j]` 会 IndexError → 被外层 `rerank` 的 `except` 吞掉静默回退原序。可接受(graceful)但无 telemetry。
- **[low] `core/search.py:202`** `r["id"].split(":")[0]` 作为 vector_result 的 item_id 回退:依赖 `doc_id = "{item_id}:{chunk}"` 约定(queue.py:93 一致),但 metadata 缺失时该回退对含 `:` 的 item_id 不健壮(uuid4().hex 无 `:`,当前安全)。
- **[info] `api/search.py:71` `search_relevant`**:每条结果都 `record_injection` 写 DB + `UPDATE use_count`,在 to_thread 搜索之后于 async handler 内**同步**循环写 SQLite(N 次 commit)。结果集大时阻塞 event loop;原型可接受。
- **[info] `main.py:174` auth_middleware**:fail-closed 设计正确(空 token 拒绝非 localhost)。`request.client.host` 在反代后是代理 IP——原型自起场景无代理,OK;若未来上反代会误判 localhost。
- **[ok] 并发/幽灵向量**:`cancel_embeddings_for_item` + `bulk_archive` 取消 pending 任务防 worker 重写已删向量,`fail_embedding` 用原子 `CASE WHEN` UPDATE——这两处是高质量正确性处理。

### (2) 复杂度热点

- `platform/detector.py` 719 LOC:**最大文件但非真热点**。内容 = 4 张芯片数据表(Intel NPU/iGPU、AMD NPU)+ 线性 `_identify_*_chip` 匹配。三个 `_identify_*` 函数结构高度相似(各 ~50 行,kernel_ok/firmware/missing/cmds 同套路),可抽公共 helper 省 ~60-80 LOC,但收益有限(数据驱动清晰)。
- `db/sqlite_db.py` 644 LOC:单类 30+ 方法,职责单一(全是 CRUD/queue/feedback),无超长函数,可接受。
- 无 >100 行函数、无 >4 层嵌套。complexity 维度整体健康。

### (3) Dead code / 未用

- **[medium] `scheduler/queue.py:117` `process_immediate`**:src 全仓 0 调用(仅自身定义)。死方法,~13 LOC。
- **[medium] `core/embedding.py:144-156` `OpenVINOEmbedding`**:构造即 `raise NotImplementedError("...Phase 4")`,`embed` 也 raise,工厂 `create_embedding_engine` 对 `device=="openvino"` 直接 warning 回退 ONNX(line 196-197)——该类**永不被实例化**。纯占位 ~13 LOC + 工厂里的 openvino 分支。
- **[medium] `api/skills.py` 全文**:`/skills` GET/POST + execute 三个端点全部 `return {"status": "not_implemented"}` + `# TODO Phase 3`。已 `app.include_router` 进生产路由表(main.py:203)。stub,~23 LOC。
- **[medium] `api/setup.py` 全文**:`/setup` 返回 `<h1>...TODO</h1>`,`# TODO Phase 5`。已注册。stub,~13 LOC。
- **[medium] DB schema 未用表**:`SCHEMA_SQL` 建了 `skills`(80-89)与 `optimization_history`(108-117) 两表,src 中**无任何 INSERT/SELECT**(`skills` 仅出现在 stub 路由文件名,非真查询;`optimization_history` 0 引用)。建表死代码 ~20 LOC + 永远空表。
- **[low] `ws.py` `/api/v1/ws`**:仅 echo `pong`,无实际下载进度推送对接(README 宣称"实时进度")。半 stub。

### (4) Schema 漂移(已建未用列)— 同时是正确性隐患

- **[medium] `sqlite_db.py:149-157`** 对 `embedding_queue` 增量迁移加了 `level`(默认2)、`section_idx`(默认0) 两列,`enqueue_embedding` 也接受 `level=2, section_idx=0` 参数 —— 但**全 src 没有任何调用传入非默认值**(grep `level=`/`section_idx=` 写入 0 处)。两列恒为默认值。配套的"章节级 Level1"切割逻辑不存在(见下 doc-drift)。

### (5) Doc-drift 清单

- **[high] CLAUDE.md `pipeline.py` 描述**:"解析→**两层入队(章节 Level1 + 段落块 Level2)**→存储→embedding"。实际 `pipeline.py:69` 只调 `self.chunker.chunk(content)` 单层滑窗分块,无章节切割,`enqueue_embedding` 全用默认 level=2。**两层索引未实现**。
- **[high] CLAUDE.md `core/search.py` 描述**:"两阶段层级检索 (search_relevant) + 动态注入预算"。实际 `search_relevant`(api/search.py)只是带 `rerank=True` + `context` 的单阶段 RRF;**无"层级/两阶段"逻辑**,无"动态注入预算"代码。
- **[high] CLAUDE.md `core/chunker.py` 描述**:"滑动窗口分块 + **extract_sections() 语义章节切割**"。实际 chunker.py **无 `extract_sections` 方法**(grep 0 命中),只有 `chunk()` + `_find_boundary()`。
- **[high] CLAUDE.md `core/search.py`**:"**两阶段层级检索 (search_relevant)**" 与 `core/parser.py` "extract_sections() 语义章节切割" 同属未实现声明。
- **[medium] CLAUDE.md `db/sqlite_db.py`**:"(含 level/section_idx)" 当真实功能描述——列在,功能不在(见 schema 漂移)。
- **[medium] README.md:83 测试数**:"测试数 13 (后端 API)"，但主仓 CLAUDE.md 写 "73 tests"。两处不一致,且与实际 `tests/` 文件量(test_extension 632 + model_integration 448 + api 228 + search 226 ...)对不上 13。
- **[low] `main.py:47,158` 版本**:hardcode `v0.1.0` / `version="0.1.0"`,而主仓已 v1.2.0。原型线 README 明示"不在 release 矩阵内",可接受不同步,但 banner 文案陈旧。
- **[low] CLAUDE.md `tray.py` / `core/parser.py parse_bytes()`**:CLAUDE.md 列了 `tray.py`(系统托盘)与 `parser.py::parse_bytes()`,需核实是否仍存(本次未在 src 顶层 LOC 列表见 tray.py)。

### (6) 安全(§1.4 secrets / 注入)

- **[ok] 无硬编码 secret**:全 src grep 无真实 key/token/password。auth token 走 `settings.auth.token`(config 注入)。
- **[ok] SQL 注入**:全部参数化查询(`?` placeholder);唯一动态 SQL 是 `update_item` 的列名,但列名来自**白名单**(line 240/243 `if k in (...)`)——安全。
- **[ok] FTS LIKE 回退**:`fts_search` 回退分支 `query[:200]` 截断防超长,且 LIKE 用 placeholder——OK。
- **[ok] CORS**:`main.py:162` 已移除 `*` 通配,改 `allow_origin_regex` 限 chrome-extension + localhost。正确收敛。
- **[info]** `model_routes.py:248` `/models/download` 直接对 Ollama `localhost:11434` 发 pull,model_name 已用 `SUPPORTED_MODELS` 白名单校验(line 238)——无任意拉取。

### (7) 依赖冗余

- **[low] `pyproject.toml`**:`openvino`(extra `npu-intel`)被引入,但唯一消费者 `OpenVINOEmbedding` 是 NotImplementedError 死类 + detector 的可选 import。可降级为纯检测探针,不需作为 embedding backend 依赖。
- **[info]** `chromadb>=0.5` 是重依赖(原型用)。Rust 线用 usearch,Python 线保留 chroma 合理(原型隔离)。
- **[info]** `jieba` README 列在技术栈,但 src 实际 FTS 用 SQLite unicode61 逐字 + `build_fts_query`(fulltext.py);需核实 jieba 是否真被 import(grep 未在已读文件见到)。可能 README 列了未实际使用的分词器。

---

## 简化 / 压缩建议(LOC 估)

| # | 动作 | 文件 | 估 LOC | 风险 |
|---|------|------|:--:|------|
| S1 | 删 `OpenVINOEmbedding` 死类 + 工厂 openvino 分支 | embedding.py | ~18 | 无(永不实例化) |
| S2 | 删 `process_immediate` 死方法 | queue.py | ~13 | 无(0 调用) |
| S3 | 删/合并 stub 路由 `skills.py`+`setup.py` 及 main.py 注册 | skills/setup/main | ~40 | 低(返回 not_implemented,无真用户) |
| S4 | 删未用表 `skills`/`optimization_history` + `level`/`section_idx` 列迁移 | sqlite_db.py | ~25 | 低(空表,无数据) |
| S5 | 抽 `_identify_intel_npu/_intel_igpu/_amd_npu` 公共 helper(kernel_ok/missing/cmds 装配) | detector.py | ~60-80 | 低(纯重构) |
| S6 | (战略)README 自承"特性已 promote 到 Rust"——评估冻结/归档原型线已毕业模块(search/vectorstore/embedding/detector 在 Rust 全有对应)。若确认原型线只保留"新算法实验沙盒",可整体瘦身 | 全 src | 数百级(策略性,需用户拍板) | 中(需确认无在用实验) |

**直接可删(S1-S4)合计 ~96 LOC**;含 S5 重构 ~160-180 LOC;S6 是战略级(与 Rust 重叠去重)需用户决策。

---

## 最大简化机会

1. **战略层(最大)**:原型线与 Rust 商用线**功能性重叠**——SQLite/FTS5/RRF 混合搜索/向量库/embedding 工厂/平台检测,Rust 全部已实装且是发布线。README 自身定位"验证有效已 promote"。**原型线应明确收敛为"纯算法实验沙盒"**,删除/冻结已毕业的生产级模仿模块,而非维护两套等价实现(避免 doc-drift 与双倍维护)。
2. **战术层**:S1-S4 直接删死代码 + 未用 schema ~96 LOC,无风险。
3. **诚信层**:修 doc-drift——CLAUDE.md/README 宣称的"两层层级索引 / extract_sections / 两阶段检索 / 动态注入预算"在 Python 端**均不存在**,要么实现要么从文档删除(当前是"文档写了功能、代码没有"的反向 drift)。

---

## Doc-drift 汇总(供修订)

| 文档位置 | 声明 | 实际 | 处置 |
|------|------|------|------|
| CLAUDE.md `pipeline.py` | 两层入队(章节L1+段落L2) | 单层 chunk(),level 恒=2 | 删声明 或 实现 |
| CLAUDE.md `chunker.py` / `parser.py` | extract_sections() 语义章节切割 | 方法不存在 | 删声明 |
| CLAUDE.md `core/search.py` | 两阶段层级检索 + 动态注入预算 | 单阶段 RRF+rerank,无预算 | 删声明 |
| CLAUDE.md `sqlite_db.py` | 含 level/section_idx | 列在功能不在 | 删列或实现 |
| README.md:83 | 测试数 13 | 与主仓"73"及实际文件量不符 | 统一计数 |
| main.py:47/158 | v0.1.0 | 主仓 v1.2.0 | 原型可不同步,banner 可更新 |
