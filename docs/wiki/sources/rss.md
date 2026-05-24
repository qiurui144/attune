---
sidebar_position: 5
---

# RSS 订阅

Attune 支持订阅 RSS / Atom Feed，将技术博客、新闻、播客等内容自动入库。

## 添加 RSS 源

1. **Settings → Sources → 添加源**，选择 "RSS"
2. 填写 Feed URL，例如：

```
https://blog.rust-lang.org/feed.xml
https://hnrss.org/frontpage
https://feeds.feedburner.com/PaulGrahamEssays
```

3. 可选配置：

| 字段 | 说明 | 默认值 |
|------|------|-------|
| 标签 | 给这个 Feed 打标签，方便检索时过滤 | 无 |
| 历史深度 | 首次拉取几条历史文章 | 50 |
| 摘要模式 | 只存 description 字段（不抓全文） | 关闭 |
| 全文抓取 | 访问原文 URL 获取全文（本地爬取，不走 LLM） | 关闭 |

## 全文抓取

当 Feed 只包含摘要时，可开启"全文抓取"：Attune 使用内置爬虫访问原文 URL，提取正文部分后入库。

> 全文抓取在本地完成（使用 chromiumoxide 驱动系统 Chrome），不借助任何云端 API，**⚡ 本地算力层**触发。

## 增量同步

Attune 基于 `pubDate` / `updated` 字段判断新文章：

- 每 **60 分钟**检查一次（可在 Settings 调整）
- 已入库的 item 按 URL + content hash 去重，不重复处理
- Feed 中的旧文章不会被二次入库

## 常用 RSS 地址参考

| 类型 | 来源 | Feed 地址 |
|------|------|----------|
| 技术博客 | Hacker News 精选 | `https://hnrss.org/best` |
| 技术博客 | Rust Blog | `https://blog.rust-lang.org/feed.xml` |
| 技术博客 | InfoQ 中文 | `https://feed.infoq.cn` |
| 播客 | 内核恐慌 | `https://kernelpanic.fm/feed.xml` |

## 播客（音频 RSS）

若 Feed 中的 enclosure 是音频文件，Attune 会自动用 **whisper.cpp** 进行 ASR 转写后入库。转写时间因文件长度而异（约 1 倍速，后台进行）。

> ASR 转写属 **⚡ 本地算力**，自动后台完成，不需要用户触发。
