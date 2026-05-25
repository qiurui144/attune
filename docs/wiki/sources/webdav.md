---
sidebar_position: 3
---

# WebDAV 云盘

Attune 支持从任何标准 WebDAV 服务器增量同步文件到本地知识库。

## 支持的服务

| 服务 | WebDAV 地址示例 | 备注 |
|------|----------------|------|
| 坚果云 | `https://dav.jianguoyun.com/dav/` | 官网可查应用密码 |
| Nextcloud | `https://your.server.com/remote.php/webdav/` | |
| Synology NAS | `https://nas.local:5006/` | 需开启 WebDAV |
| ownCloud | `https://your.server.com/remote.php/webdav/` | |
| 通用 WebDAV | 任意符合 RFC 4918 的服务 | |

## 添加 WebDAV 源

1. **Settings → Sources → 添加源**，选择 "WebDAV"
2. 填写配置项：

```
服务器地址：https://dav.jianguoyun.com/dav/
用户名：your@email.com
密码：（应用专用密码，非登录密码）
远端路径：/research/papers/      ← 只同步这个子目录，留空则同步根目录
```

3. 点击"测试连接"验证配置
4. 保存后 Attune 立即进行首次全量扫描

## 增量同步机制

Attune 的 WebDAV worker 使用 `Last-Modified` + `ETag` 双重比对：

- 每 **15 分钟**检查一次远端变更（可在 Settings 调整间隔）
- 新增文件 → 下载 + ingest
- 内容变更文件 → 重新下载 + 增量 reindex（旧 chunks 先删）
- 远端删除文件 → 本地知识库对应条目标记为"已归档"（不立即删除，防误操作）

## 凭据安全

WebDAV 密码使用 **Argon2id 派生密钥 + AES-256-GCM** 字段级加密，保存在 vault 数据库中。Vault 只有你输入主密码解锁后才可读取。

凭据**不会**同步到任何 Attune 服务器。

## 常见问题

**Q: 坚果云提示 403 Forbidden？**

坚果云 WebDAV 不支持账户密码，需要在坚果云网页端 → "账户信息 → 安全选项"创建**应用密码**后使用。

**Q: 同步速度很慢？**

WebDAV 同步受限于远端服务器的下行速度和 Attune 的 ingest 队列处理速度。大量文件首次同步时，可在顶栏"后台任务"面板查看进度。已存在且内容未变的文件会直接跳过。

**Q: 如何只同步特定子目录？**

在"远端路径"填写具体子目录，例如 `/Documents/工作/2026/`。Attune 只扫描该路径下的文件，不影响 WebDAV 上的其他目录。
