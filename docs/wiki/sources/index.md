---
sidebar_position: 1
---

# 数据采集源（Sources）

Attune 通过多种采集源将文件和内容持续同步到本地知识库。所有数据**在落库前先经过 L1 PII 脱敏**，不向云端传输原始文件。

## 支持的采集源

| 类型 | 说明 | 同步方式 |
|------|------|---------|
| [本地文件 / 文件夹](./local-files.md) | 手动上传 或 监听目录 | 实时 (watchdog) |
| [WebDAV 云盘](./webdav.md) | NAS / 坚果云 / Nextcloud | 周期增量 |
| [Email (IMAP)](./email.md) | Gmail / Outlook / QQ 邮箱等 | 周期拉取 |
| [RSS 订阅](./rss.md) | 技术博客 / 新闻 / 播客 | 周期拉取 |

## 统一限制

- 单文件最大 100 MB（v1.0）
- 支持格式：PDF / DOCX / TXT / MD / 代码文件 / 图片（OCR）/ 音频（ASR）
- 所有采集源共用**同一个后台 ingest 队列**；队列状态可在顶栏"后台任务"面板查看

## 采集源管理

进入 **Settings → Sources** 可以：

- 添加 / 编辑 / 删除采集源
- 查看每个源的最后同步时间 + 已索引文件数
- 手动触发一次全量重同步

---

> **隐私说明**：WebDAV / Email / RSS 的远端凭据使用 Argon2id + AES-256-GCM 字段级加密保存在本地 vault，不上传到任何 Attune 服务器。详见 [隐私模型](../privacy.md)。
