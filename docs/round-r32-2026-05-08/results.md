# Attune OSS — Round 32 Plugin Lifecycle (marketplace 写操作)

**Started**: 2026-05-08 09:09

**目标**: 之前 R20 发现 plugin marketplace 写操作 toggle/enable/disable 全 404 (read-only listing)。R32 探索 v0.6.x 路由表是否有改进 + 用户级 plugin 操作能否走通。


## ⭐ R32 Plugin Lifecycle E2E

### 路由表实测 (修正 R20 错误方法论)

| Endpoint | Method | Status | Result |
|----------|--------|--------|--------|
| `/api/v1/plugins` | GET | 200 | OSS 无 builtin (空 list) |
| `/api/v1/plugins/:id/toggle` | POST | 200 | settings.plugins.disabled 数组操作 ✓ |
| `/api/v1/marketplace/plugins` | GET | 200 | 4 mock plugins (law/patent/presales/tech) |
| `/api/v1/marketplace/plugins/:id/install` | POST | 200 | Mock provider returns install metadata |

### Install metadata 示例
```json
{
  "install_id": 1,
  "plugin_id": "law-pro",
  "version": "0.7.0",
  "sha256": "mock-sha256",
  "trial_started": null,
  "trial_expires": null,
  "download_url": "/api/v1/packages/law-pro-0.7.0.tar.gz"
}
```

### Toggle 状态机
- 1st toggle: enabled=false → settings.plugins.disabled: ["law-pro"]
- 2nd toggle: enabled=true → settings.plugins.disabled: []
- 单一 toggle endpoint flip (无独立 enable/disable)

### 修正 R20 错误方法论
之前 R20 测 `/marketplace/plugins/:id/{toggle,enable,disable}` 全 404 是**路径错误**:
- 真实 toggle 在 `/plugins/:id/toggle` (无 marketplace 前缀)
- 没有独立 enable/disable - toggle endpoint 单一 flip
- 这是测试时对路由表理解不充分导致的误判

### 行业 plugin (law-pro / patent-pro / presales-pro / tech-pro) 的位置
- OSS 仓: marketplace mock provider 返回 4 名行业 plugin metadata
- 实际 plugin code: attune-pro 私有仓
- 安装路径: marketplace install → 需注入 attune-pro hub-client 才能真下载执行
- 当前 OSS 仓 install 只返回 metadata 不真实下载 (Mock 设计正确)


## R32 Extra 180min sustained
**Wall time**: 10800s — 10634/10634 ok
