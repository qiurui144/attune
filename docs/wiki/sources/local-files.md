---
sidebar_position: 2
---

# 本地文件 / 文件夹

## 手动上传

最简单的入口：在 Web UI 的文件标签页，把文件**拖拽到上传区**或点击"选择文件"。

支持批量上传（Ctrl/Cmd + 单击多选）。上传后文件进入 ingest 队列，后台自动完成：

1. 文件解析（PDF → 文本 / DOCX → Markdown / 图片 → OCR 文本 / 音频 → ASR 转写）
2. 分块（滑动窗口 + 语义章节切割）
3. Embedding 生成（本地 bge 模型）
4. 全文索引（tantivy jieba 分词）
5. 生成 150 字存档摘要（本地算力，非 LLM）

**不需要等待**：上传成功即可开始搜索（FTS 立即可用），向量检索在 embedding 完成后自动激活。

## 文件夹绑定与重扫

对于本地的工作目录，可以绑定为本地知识库 source：

1. **Settings → Sources → 添加源**，选择"本地目录"
2. 输入或选择路径（例如 `~/Documents/research`）
3. 可选设置排除规则（支持 glob，如 `*.tmp` / `.git/**`）

文件夹绑定后有两种更新入口：

- 周期重扫 worker：默认每 30 分钟扫描一次活跃绑定目录。
- 显式重扫：调用 `POST /api/v1/index/rescan`，传入 `dir_id`，NAS Web 可用它在用户点击刷新时立即同步。

### 排除规则示例

```
*.tmp
*.log
node_modules/**
.git/**
__pycache__/**
```

## 内容 Hash 去重

Attune 对每个文件计算 SHA-256 内容 hash。**相同内容的文件不会重复索引**，即使文件名或路径不同。当某个文件内容未变更时，ingest 流程直接短路，不消耗 embedding。

本地目录还会记录文件大小、mtime、ctime、inode/dev 等 stat marker。marker 完全一致且原 item 仍有效时，下一次 rescan 直接跳过文件读取和 SHA-256，适合 NAS 或大目录快速刷新。

## 删除文件

文件从绑定目录中删除后，下一次 rescan 会自动：

- 将对应 item 标记删除
- 入队 purge，清理向量索引和全文索引
- 删除该 source 的 `indexed_files` tracking 行

手动在文件列表右键 → "从知识库移除" 仍只影响 Attune 知识库，不删除原始文件。

## 标记为机密（L0）

右键文件 → "标记为 🔒 机密"，该文件的 chunks 在 Chat 检索阶段被过滤，永不出现在 LLM 上下文中。详见 [隐私模型](../privacy.md)。
