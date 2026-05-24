---
sidebar_position: 4
---

# Email（IMAP）

Attune 可以通过 IMAP 协议接入邮箱，将邮件正文和附件持续同步到知识库。

## 支持的邮箱

| 邮箱 | IMAP 服务器 | 端口 |
|------|-----------|------|
| Gmail | `imap.gmail.com` | 993 (SSL) |
| Outlook / Hotmail | `outlook.office365.com` | 993 (SSL) |
| QQ 邮箱 | `imap.qq.com` | 993 (SSL) |
| 163 邮箱 | `imap.163.com` | 993 (SSL) |
| 企业邮箱 (Exchange) | 联系管理员获取 IMAP 地址 | 993 |

## 添加 Email 源

1. **Settings → Sources → 添加源**，选择 "Email"
2. 填写配置项：

```
IMAP 服务器：imap.gmail.com
端口：993
用户名：your@gmail.com
密码：（应用专用密码）
监听文件夹：INBOX              ← 可指定多个，逗号分隔
最早拉取时间：2026-01-01        ← 不填则从 30 天前开始
附件类型：PDF, DOCX, TXT        ← 留空则忽略附件
```

3. 点击"测试连接"

## Gmail 应用专用密码

Gmail 默认不允许直接用账户密码连接 IMAP（即使开启了 2FA），需要创建**应用专用密码**：

1. 前往 [Google 账户安全设置](https://myaccount.google.com/security)
2. 找到"两步验证" → "应用专用密码"
3. 选择"其他"，输入名称（如 "Attune"），生成 16 位密码
4. 将该密码填入 Attune 的 Email 配置

## 同步范围

Attune 默认按以下规则筛选：

- **正文**：纯文本或 HTML（Attune 自动剥离 HTML 标签）
- **附件**：仅支持格式内的附件（PDF / DOCX / TXT / MD 等），不下载图片附件（除非你显式添加图片格式）
- **发件时间**：按配置的"最早拉取时间"过滤，之后的邮件全量入库

## 增量更新

Attune 使用 IMAP 的 `UID SEARCH SINCE` 机制：

- 每次同步记录已处理的最大 UID
- 下次只拉取 UID 更大（即更新）的邮件
- 默认每 **30 分钟**同步一次（可在 Settings 调整）

## 隐私说明

- 邮件内容在入库前经过 L1 PII 脱敏（手机号 / 邮箱 / 地址等替换为 placeholder）
- IMAP 密码使用与 WebDAV 相同的字段级加密保存
- Attune 不向任何服务器上传邮件内容，所有处理在本地完成
