# Attune 分发 + 合规 — 自动更新 · 签名包 · 诊断导出 · Telemetry · Legal

**Date:** 2026-04-19
**Status:** 待实施（发布前冲刺阶段）
**Scope:** Attune 对外分发 + 法律合规基础设施
**Parallel to:** `2026-04-19-frontend-redesign-design.md` · `2026-04-19-ux-quality-design.md` · `2026-04-19-data-infrastructure-design.md`
**Part of:** "产品级基础框架" 系列 3 个并行 spec 中的 2c

---

## 0. 背景与动机

Attune 是私有 AI 知识伙伴，用户把几年的专业知识放进去。**公开发布前**必须补齐：

1. **分发可信** —— 签名包 + 校验清单，用户装得放心
2. **自更新通路** —— 发 bugfix / 安全补丁用户能收到
3. **诊断可导** —— 用户遇到 bug 一键打包发支持
4. **合规边界** —— 法律底线：privacy policy + ToS + 第三方 license + SBOM

### 本 spec 范围

| 编号 | 子系统 | 优先级 |
|------|-------|------|
| **A4** | 自动更新通知 + 下载引导 | Must-have |
| **B1** | 签名分发包（macOS notarize / Windows sign / Linux GPG） | Must-have |
| **B2** | 第三方 license 聚合 + About 展示 | Must-have |
| **B3** | 诊断包一键导出 | Must-have |
| **B8** | 匿名 telemetry opt-in（默认关） | Optional MVP |
| **L1** | Privacy Policy 页面 + 内嵌 | Must-have |
| **L2** | Terms of Service | Must-have |
| **L3** | SBOM 生成（CycloneDX） | Must-have |
| **L4** | Export Control 声明（密码学） | Must-have |
| **L5** | DMCA / 商标 | Optional（视是否接受社区插件） |

### 时机

此 spec 在**发布前 4-6 周冲刺**启动，与前 3 个 spec（UI / UX / 数据）协调落地。

---

## 1. A4 · 自动更新机制

### 设计原则

- **不自动安装**（用户不爽被迫更新），只**通知 + 引导下载**
- **本地不做 auto-patch** 这种高风险操作（个人设备 + 加密 vault 混合出错代价极高）
- **更新源可配置**（公司内网可指向自托管）

### 版本检查

启动后 + 每 24 小时：

```
GET https://releases.attune.ai/v1/latest.json
Accept: application/json

Response:
{
  "version": "0.7.0",
  "released_at": "2026-05-15T00:00:00Z",
  "urgency": "normal|recommended|critical",
  "min_supported_version": "0.5.0",
  "changelog_url": "https://attune.ai/releases/0.7.0",
  "downloads": {
    "linux-x86_64": {
      "url": "https://releases.attune.ai/v1/0.7.0/attune-0.7.0-linux-x86_64.tar.gz",
      "sha256": "abc...",
      "signature_url": "https://releases.attune.ai/v1/0.7.0/attune-0.7.0-linux-x86_64.tar.gz.sig",
      "size_bytes": 30123456
    },
    "linux-aarch64": { ... },
    "macos-x86_64": { ... },
    "macos-aarch64": { ... },
    "windows-x86_64": { ... }
  }
}
```

比较 `latest.version > current.version`（语义化版本） → 需要更新。

### UI 表现

- **普通更新**（urgency: normal）：sidebar 账户头像右上角红点 + Settings > 关于页面看到
- **推荐更新**（urgency: recommended）：首次启动时右下角 toast 弹一次，不强求
- **关键更新**（urgency: critical，通常是安全修复）：启动时全屏 modal + 强烈建议，但有 "稍后提醒" 逃生门

### 下载流程

用户点击"更新" → 不直接下载，打开浏览器指向 changelog + download 页：

```
https://attune.ai/releases/0.7.0
  显示：
  - 本次更新内容
  - 平台对应 download 链接
  - SHA-256 校验值
  - 签名校验指南
  - 用户手动下载 → 解压/安装 → 替换老 binary
```

**不在 Attune 内部下载更新包**，理由：
- 避免把 Attune 变成"更新器"（简化产品边界）
- 浏览器下载 + OS 安装流程用户更熟悉
- 手动替换强制用户做备份，降低出错率

### 配置

```json
"update": {
  "check_enabled": true,
  "release_channel": "stable",   // stable | beta (未来)
  "update_url": "https://releases.attune.ai/v1/latest.json",
  "last_check": "2026-04-19T10:00:00Z",
  "last_version_seen": "0.6.0"
}
```

企业内网可改 `update_url` 指向私有 mirror。

### 实现

- 新模块：`attune-core/src/updater.rs`（~150 行，纯 HTTP client + semver 比较）
- 新 API：
  ```
  GET /api/v1/update/status   -> { current, latest, available, urgency, changelog_url, downloads }
  POST /api/v1/update/check   -> 触发即时检查
  ```
- 后台任务：每 24h 调一次（在 SkillEvolver 旁边加 task），网络失败静默重试

### 首次安装的特殊情况

- Wizard Step 1 之前：跑一次 update check，若发现当前版本远低于 latest → 提示"你下载的不是最新版" + 链接
- 正常情况不阻塞 wizard

---

## 2. B1 · 签名分发包

### 平台策略

| 平台 | 签名方式 | 成本 | MVP 计划 |
|------|---------|------|---------|
| **Linux** | GPG sign .deb/.rpm/.tar.gz + SHA-256 sums | $0 | ✅ MVP 就做 |
| **macOS** | Apple Developer ID + notarization | $99/年 | ⚠ 发布时必做（否则 Gatekeeper 拦截） |
| **Windows** | EV code signing cert | $300-400/年 | 🟡 MVP 可用自签 + SmartScreen 警告，商用时升 EV |

### Linux · GPG 签名

**CI pipeline（`rust-release.yml` 扩展）**：

```yaml
- name: GPG sign artifacts
  env:
    GPG_PRIVATE_KEY: ${{ secrets.GPG_PRIVATE_KEY }}
    GPG_PASSPHRASE: ${{ secrets.GPG_PASSPHRASE }}
  run: |
    echo "$GPG_PRIVATE_KEY" | gpg --batch --import
    for file in attune-*.tar.gz attune-*.deb attune-*.rpm; do
      gpg --batch --passphrase "$GPG_PASSPHRASE" \
          --armor --detach-sign "$file"
      sha256sum "$file" > "$file.sha256"
    done

- name: Upload artifacts to release
  uses: softprops/action-gh-release@v2
  with:
    files: |
      attune-*.tar.gz
      attune-*.tar.gz.asc
      attune-*.deb
      attune-*.deb.asc
      attune-*.rpm
      attune-*.rpm.asc
      attune-*.sha256
```

**用户校验指南**（附在 release notes）：

```bash
# 1. 下载 attune-0.7.0-linux-x86_64.tar.gz 和 .asc 签名文件
# 2. 导入 Attune 公钥（一次性）
curl -sSL https://attune.ai/pgp-key.asc | gpg --import

# 3. 校验签名
gpg --verify attune-0.7.0-linux-x86_64.tar.gz.asc

# 4. 校验 SHA-256
sha256sum -c attune-0.7.0-linux-x86_64.tar.gz.sha256
```

### macOS · Notarization

需求：
- 注册 Apple Developer Program（$99/年）
- 生成 Developer ID Application 证书
- 在 CI 里用 `xcrun notarytool` 上传给 Apple 验证
- 成功后 staple 到 .dmg

**CI 流程**：

```yaml
- name: Build .dmg
  run: |
    cargo build --release --target ${{ matrix.target }}
    ./scripts/create-dmg.sh target/release/attune attune-0.7.0-macos.dmg

- name: Codesign binary
  env:
    CODESIGN_ID: ${{ secrets.MACOS_CODESIGN_ID }}
    KEYCHAIN_PWD: ${{ secrets.MACOS_KEYCHAIN_PWD }}
  run: |
    # 解锁 keychain、codesign binary、codesign .dmg
    security unlock-keychain -p "$KEYCHAIN_PWD" build.keychain
    codesign --deep --force --sign "$CODESIGN_ID" \
             --options runtime --entitlements entitlements.plist \
             target/release/attune
    codesign --force --sign "$CODESIGN_ID" attune-0.7.0-macos.dmg

- name: Notarize
  env:
    APPLE_ID: ${{ secrets.APPLE_ID }}
    APPLE_PASSWORD: ${{ secrets.APPLE_PASSWORD }}
    TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
  run: |
    xcrun notarytool submit attune-0.7.0-macos.dmg \
          --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" \
          --team-id "$TEAM_ID" --wait

- name: Staple
  run: xcrun stapler staple attune-0.7.0-macos.dmg
```

### Windows · Code Signing

**MVP（无 EV 证书）**：
- 不签名
- SmartScreen 会警告用户 "未知发布者"
- 用户需点 "更多信息" → "仍要运行"
- 安装指南清晰描述此步骤

**商用升级（EV cert 后）**：
```yaml
- name: Sign .exe and .msi
  env:
    SIGNTOOL_CERT_THUMBPRINT: ${{ secrets.WINDOWS_CERT_THUMBPRINT }}
  run: |
    signtool sign /sha1 "$SIGNTOOL_CERT_THUMBPRINT" /tr http://timestamp.digicert.com \
             /td sha256 /fd sha256 attune-0.7.0-windows.exe
    signtool sign /sha1 "$SIGNTOOL_CERT_THUMBPRINT" /tr http://timestamp.digicert.com \
             /td sha256 /fd sha256 attune-0.7.0-windows.msi
```

### CI 矩阵

更新 `rust-release.yml`：

```yaml
strategy:
  matrix:
    include:
      - os: ubuntu-latest
        target: x86_64-unknown-linux-gnu
        sign: gpg
      - os: ubuntu-latest
        target: aarch64-unknown-linux-gnu
        sign: gpg
        cross: true
      - os: macos-14
        target: x86_64-apple-darwin
        sign: apple
      - os: macos-14
        target: aarch64-apple-darwin
        sign: apple
      - os: windows-2022
        target: x86_64-pc-windows-msvc
        sign: ev           # MVP 阶段改 none
```

### Release Notes 模板

每个 release 附：
- 此版本变更摘要（从 CHANGELOG.md 抽取）
- 各平台下载链接
- SHA-256 列表
- 签名校验指南
- Migration notes（若本版本有 DB migration）

---

## 3. B2 · 第三方 License 聚合

### 工具链

**Rust 侧**：
- `cargo-about`（生成 THIRD_PARTY_LICENSES.html）
- 配置：`about.toml` 指定允许的 license 白名单（MIT/Apache-2.0/BSD-3-Clause/ISC/MPL-2.0）
- CI step：每次 release 自动生成

**前端侧**：
- `license-checker --production --onlyAllow 'MIT;Apache-2.0;BSD-3-Clause;ISC;MPL-2.0' --json`
- 生成 `npm-licenses.json`，合并到 Rust 侧 HTML

### 产物

`THIRD_PARTY_LICENSES.html`（~500KB，250+ 依赖）：
- 每个依赖列出：名称 + 版本 + license 名 + license 全文
- 嵌入 Rust binary 通过 `include_str!`
- 在 Settings > 关于 > 第三方许可中展示

### CI step

```yaml
- name: Generate third-party licenses
  run: |
    cargo install cargo-about
    cargo about generate -c rust/about.toml \
                         -o rust/crates/attune-server/assets/THIRD_PARTY_LICENSES.html \
                         rust/about-template.hbs
    cd rust/crates/attune-server/ui
    npm install
    npx license-checker --production --onlyAllow '...' --json > licenses.json
    # merge two into one HTML（自写合并脚本）

- name: Fail if license violation
  run: cargo about check
```

### UI

Settings > About:
- 展示 Attune 版本 + Apache-2.0 声明
- 展示 Fjord Teal/UI 素材作者致谢（如使用开源图标库如 Lucide）
- CTA："查看第三方许可详情" → 打开内嵌 HTML（不是外链）

---

## 4. B3 · 诊断包一键导出

### 触发路径

- Settings > 诊断 > "导出诊断包" 按钮
- 或崩溃上报提示条里的"发送诊断包给支持"快捷按钮

### 内容清单

诊断 zip 包含（由 2a 预留的 `/api/v1/diagnostic-data` 聚合）：

| 文件 | 内容 | 脱敏 |
|------|------|------|
| `system.json` | OS / CPU / RAM / GPU / NPU 信息 | 无 |
| `version.json` | Attune 版本 + Cargo.lock hash + npm lock hash | 无 |
| `settings.json` | 全部 settings | redact api_key / master password hash |
| `migrations.json` | 已应用的 migration 列表 | 无 |
| `crashes/*.json` | 最近 100 条崩溃报告（2a） | 已在 2a 脱敏 |
| `server.log` | 最近 1 小时 server 日志 | redact IP / 用户名替换 |
| `client-errors.json` | 最近 100 条客户端 error | 脱敏 |
| `hardware.json` | `GET /diagnostics` 输出 | 无 |
| `backup-manifests.json` | 所有备份的 manifest（不含备份内容） | 无 |

**不含**：
- vault.db / 用户知识内容
- 批注 / chat 历史
- API keys 明文
- Master password 或 derived keys

### 生成流程

```rust
pub fn generate_diagnostic_bundle() -> Result<PathBuf> {
    let temp_dir = tempfile::tempdir()?;
    
    // 1. 收集各项 JSON
    write_json(temp_dir.join("system.json"), &detect_hardware())?;
    write_json(temp_dir.join("version.json"), &version_info())?;
    write_json(temp_dir.join("settings.json"), &redacted_settings()?)?;
    // ...
    
    // 2. 拷贝 crashes/ 和 logs/
    copy_recent_crashes(temp_dir.join("crashes"), 100)?;
    copy_recent_logs(temp_dir.join("server.log"), Duration::from_secs(3600))?;
    
    // 3. 脱敏 server.log
    redact_log_in_place(&temp_dir.join("server.log"))?;
    
    // 4. zip
    let zip_path = platform::downloads_dir()
        .join(format!("attune-diagnostic-{}.zip", timestamp()));
    zip_directory(&temp_dir, &zip_path)?;
    
    Ok(zip_path)
}
```

### UI 表现

点击"导出诊断包" →
1. 模态：说明 zip 内容 + 脱敏声明 + 列出文件清单
2. 确认 → 进度条 → 生成完成
3. 浏览器下载 OR OS 文件选择器让用户选保存位置
4. 复制模板 subject："Attune Diagnostic Report {version} · {timestamp}"

### 隐私提示

导出模态顶部显著标注：
> 📦 诊断包**仅包含技术信息**，不含你的知识库内容、密码或 API 密钥。
> 完整清单见下方；你可以在发送前用任何 zip 工具检查内容。

### 实现

- 新模块：`attune-core/src/diagnostic.rs`
- 新 API：
  ```
  POST /api/v1/diagnostic/generate   -> { zip_path, size_bytes, preview_listing }
  GET  /api/v1/diagnostic/listing    -> 列出会被打包的内容（不生成，仅预览）
  ```

---

## 5. B8 · 匿名 Telemetry（opt-in，默认关）

### 设计原则

- **默认关闭**，wizard Step 5 完成时明确提示用户可开启
- **纯计数**，不含任何内容
- **自托管**，发到 `https://telemetry.attune.ai/v1/events`（Cloudflare Workers 或简单 nginx + SQLite）
- **可随时关闭**，一键 off 且立即生效
- **完全本地 queue**，离线不丢事件

### 收集内容（白名单）

```ts
type TelemetryEvent = {
  event_id: string;     // UUID，不关联用户
  session_id: string;   // 每次启动一个新的（不跨启动追踪）
  timestamp: string;
  event: TelemetryEventName;
  properties?: Record<string, string | number | boolean>;
};

type TelemetryEventName =
  | 'app_started'
  | 'wizard_completed'
  | 'vault_unlocked'
  | 'chat_sent'           // 只计数，不含 message
  | 'item_uploaded'       // 只计数，不含内容
  | 'plugin_installed'    // 只记 plugin_id（公开的）
  | 'crash_reported'      // 只记 crash kind（非内容）
  | 'feature_clicked'     // 记 feature_name（Reader / Knowledge 等）
  | 'app_closed';
```

**绝不收集**：
- IP 地址（server 侧自动丢弃 X-Forwarded-For）
- User-Agent 详细信息（只取大类：Chrome/Firefox/Safari/Edge）
- 文件名、chat 内容、批注内容
- 硬件序列号、MAC 地址
- 任何 PII

### 匿名 ID 生成

- 本地生成 `device_id = sha256(install_timestamp + random)`（36 字节）
- 存在 `app_settings.telemetry.device_id`
- 用户重装 Attune → 新 device_id（等价于新设备）
- 不跟 `device_secret` 关联（彻底隔离）

### 启动流程

Wizard Step 5 完成前加一个"telemetry 同意"小卡：

```
┌──────────────────────────────────────────────┐
│ 📊 帮助我们改进 Attune（可选）                │
│                                              │
│ Attune 完全本地运行，但如果你愿意，可以发送    │
│ 匿名使用统计帮我们了解哪些功能被用。           │
│                                              │
│ ☑ 我同意发送匿名统计                         │
│                                              │
│ [查看完整清单]  [稍后在 Settings 中决定]      │
└──────────────────────────────────────────────┘
```

**默认不勾选**。用户显式勾选才启用。

### Batching & 上传

- 事件本地 queue（sqlite 的 `telemetry_queue` 表）
- 每小时或每 100 条触发一次 flush
- 失败 → 保留 queue，下次 retry
- 上传超过 1000 条 queue 溢出 → 丢弃最老

### 服务端

最小 endpoint：

```
POST https://telemetry.attune.ai/v1/events
Body: { events: [TelemetryEvent, ...] }
Response: 204 No Content
```

- 后端：Cloudflare Workers + D1（SQLite）或自托管 nginx + 简单 Rust 服务
- 存储：2 年保留，之后 drop
- 查询：仅 Attune 团队 analytics dashboard（Grafana/Metabase），不对外

### 用户视角

Settings > 隐私 > Telemetry:
- 开关 toggle
- 显示：device_id（可重新生成）
- 显示：最近 7 天发送的事件计数 + 类型统计
- "查看已发送数据"：显示本地 queue + 最近已发送
- "清空本地队列"
- "撤销同意"：关闭 + 清 device_id

### 实现

- 新模块：`attune-core/src/telemetry.rs`（~200 行）
- 新 Settings 字段 `telemetry.*`
- 新服务端（独立 deploy）：`infrastructure/telemetry-server/`（未来独立仓库）

---

## 6. Legal 基础

### L1 · Privacy Policy

**内容模板**（Markdown，嵌入应用 + 公开网页）：

```markdown
# Attune 隐私政策

**最后更新：2026-05-15**

## TL;DR

Attune 是**本地运行**的知识库，**不收集**你的知识内容、聊天记录、API 密钥或密码。
所有数据加密存在你自己的设备。

## 我们不收集

- 你的知识库内容（文件、批注、聊天）
- 你的 API 密钥或 vault 密码
- 你的 IP 地址或位置
- 任何可识别你身份的信息

## 我们可能收集（仅当你显式同意）

如果你在 Settings 中开启 **Telemetry**（默认关闭），我们会收集：
- 匿名设备 ID（随机 UUID，跟你的身份无关联）
- 使用统计：启动次数、功能点击次数、崩溃类型计数
- 完整清单见 [Telemetry 说明](./telemetry.md)

你随时可关闭 Telemetry 并删除已上传数据。

## 数据安全

- 所有本地数据用 Argon2id + AES-256-GCM 加密
- Master password 永不离开设备
- 备份完全本地，除非你手动上传到云盘

## 联系

- 问题或请求：support@attune.ai
- 数据导出请求：见 Settings > 诊断 > 导出诊断包（本地操作）
- 数据删除：卸载 Attune + 删除 `~/.local/share/attune/` 即可
```

公开 URL：`https://attune.ai/privacy`
应用内：Settings > 关于 > 隐私政策（内嵌 HTML，不外链）
Wizard 末尾：引用链接，用户可点查看但不强制阅读

### L2 · Terms of Service

```markdown
# Attune 使用条款

**最后更新：2026-05-15**

## 1. 授权

Attune Core（attune-core / attune-server / attune-cli）依 **Apache License 2.0** 开源。
商业插件（标识为 "Pro"）依**专有许可**，见各插件附带声明。

## 2. 用户责任

- 妥善保管 Master Password。**我们无法找回**你的密码，遗失 = 永久数据丢失
- 自行备份。Attune 提供自动备份工具，你负责监督其正常运行
- 不使用 Attune 处理违反你所在司法管辖区法律的内容

## 3. 免责

软件按"原样"提供，不附带任何明示或默示保证。
作者不对因使用或不能使用本软件造成的任何损失负责。

## 4. 导出管制

Attune 使用密码学（Argon2id / AES-256-GCM / Ed25519），属 ECCN 5D002 类别，
免除 License Exception ENC 要求（开源）。见 NOTICE 详细。

## 5. 商标

"Attune"、"Fjord Teal" 是 Attune 团队的商标，未经授权不得用于商业场合。
开源 fork 命名必须区别于原名。

## 6. 变更

条款更新会通过 CHANGELOG.md 公告。继续使用即视为接受新条款。
```

### L3 · SBOM（Software Bill of Materials）

**工具**：
- `cargo cyclonedx` → `sbom-rust.xml`
- `@cyclonedx/bom` (npm) → `sbom-ui.xml`
- 合并为一份 `sbom.xml`（CycloneDX 1.4 格式）

**CI step**：

```yaml
- name: Generate SBOM
  run: |
    cargo install cargo-cyclonedx
    cd rust && cargo cyclonedx --format xml
    cd crates/attune-server/ui
    npx @cyclonedx/bom -o ui-sbom.xml
    ./scripts/merge-sbom.sh rust/bom.xml ui/ui-sbom.xml > sbom.xml

- name: Attach SBOM to release
  uses: softprops/action-gh-release@v2
  with:
    files: sbom.xml
```

用途：企业采购合规、漏洞追踪（CVE 比对）、第三方审计。

### L4 · Export Control 声明（NOTICE 已包含 cryptography）

在 `NOTICE` 文件追加：

```
## Cryptography Notice

This distribution includes cryptographic software. The country in which you
currently reside may have restrictions on the import, possession, use, and/or
re-export of encryption software.

Attune uses:
- Argon2id (password hashing)
- AES-256-GCM (symmetric encryption)
- Ed25519 (digital signatures, plugin signing)
- HMAC-SHA256 (integrity checking)

These algorithms are implemented via third-party crates (argon2, aes-gcm,
ed25519-dalek, hmac, sha2) which are available under their respective licenses.

This software is classified under ECCN 5D002 and is eligible for export under
License Exception ENC as defined in Section 740.17 of the U.S. Export
Administration Regulations (EAR), subsection (b)(1). This software may be
exported to most destinations without requiring export licenses, but please
check your local regulations.
```

### L5 · DMCA / 商标（Optional）

**DMCA Designated Agent**（只有接受用户内容或社区插件时才需要）：
- 通过 [U.S. Copyright Office Form](https://www.copyright.gov/dmca-directory/) 登记
- 费用：$6 一次性 + 年费
- 在 privacy policy 里标明 agent 联系方式

**商标注册**：
- 中国：商标局（~5000 元 + 6-12 个月审批）
- 美国：USPTO（~$350-$750 + 1-2 年审批）
- **决策**：先保持未注册状态，发布后见热度再决定是否注册
- 法律咨询：建议律师 review 后再执行

**开源 Fork 约束**：
- Fork 允许，但品牌名必须改（不能叫 Attune）
- 在 Apache-2.0 基础上加 trademark clause（或单独 TRADEMARK.md）

---

## 7. 成功标准

### 功能验收

- [ ] 每日启动检查 update，发现新版本 sidebar 红点
- [ ] Linux 发布产出 GPG 签名 + SHA-256，校验指南清晰
- [ ] macOS 发布产出 notarize 过的 .dmg，Gatekeeper 不拦
- [ ] Windows 发布（至少 MVP 阶段）有安装指南告诉用户跳过 SmartScreen
- [ ] Settings > 关于显示完整第三方 license HTML
- [ ] "导出诊断包" 一键产生 zip，内容清单符合 whitelist
- [ ] Telemetry 默认关闭，开启后能看到发送的数据计数
- [ ] Privacy Policy 页面可从应用内访问 + 公开 URL
- [ ] ToS + Privacy + NOTICE + SBOM 都在 release artifact 里

### 合规指标

- Apache-2.0 第三方 license 覆盖率 100%（cargo about check 无 violation）
- SBOM 包含所有传递依赖
- Privacy Policy 律师 review 过

---

## 8. 范围外（其他 spec 或发布后）

- **B1 Windows EV 证书**：MVP 不做，见热度再决定（~$400/年）
- **C 层 feature flags**（灰度发布）
- **国际化 privacy policy**（除中英外其他语言） defer
- **GDPR compliance 深化** → 如进入欧洲市场时补
- **SOC2 / ISO 27001** → 企业版才需要
- **DMCA 商标注册** → 律师 review 后决策

---

## 9. 开放问题

1. **Telemetry 是否在 MVP 就做**？
   - 做：用户行为数据宝贵
   - 不做：MVP 简化，专注核心体验
   - **决策**：MVP 做但默认关闭 + 简单版（只 5 类事件）
2. **Windows EV 证书时机**？
   - 发布后观察 SmartScreen 警告是否阻止用户（反馈渠道）
   - Pro 订阅上线后考虑买（能分摊成本）
3. **更新源自托管 vs GitHub Releases**？
   - GitHub Releases：免费、可靠，但国内访问慢
   - 自托管：快但要维护
   - **决策**：MVP 用 GitHub Releases，观察国内体验后再自建 CDN mirror
