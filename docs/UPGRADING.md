# Attune 升级指南 (UPGRADING.md)

> **SSOT 范围**:本文档是 attune (OSS) + attune-pro plugin pack 升级路径的唯一权威来源。
> Cloud / official-web / wiki 升级见 `cloud/CLAUDE.md` + `cloud/docs/RELEASE.md`。

## 目录 (Table of Contents)

- [0. 适用范围 + 红线](#0-适用范围--红线)
- [1. Pre-flight check (升级前自检)](#1-pre-flight-check-升级前自检)
- [2. Desktop App (Tauri auto-updater)](#2-desktop-app-tauri-auto-updater)
- [3. CLI / Server tarball (高阶用户)](#3-cli--server-tarball-高阶用户)
- [4. Cloud SaaS (maintainer 内部 ops)](#4-cloud-saas-maintainer-内部-ops)
- [5. attune-pro Plugin Pack](#5-attune-pro-plugin-pack)
- [6. v0.7 → v1.0 老用户特殊路径](#6-v07--v10-老用户特殊路径)
- [7. Disaster Recovery 链入口](#7-disaster-recovery-链入口)
- [8. 已知风险 + Known Limitations](#8-已知风险--known-limitations)

---

## 0. 适用范围 + 红线

**适用**:从 `v1.0.0` 起任何 attune (OSS) 桌面/CLI/server 升级。
v0.x → v1.0 老用户参 §6。

**红线(任何升级路径必须遵守)**:

- ❌ **不允许跳过 pre-upgrade backup**(per §1)— vault.db 是用户最珍贵数据,迁移失败 = 数据丢失
- ❌ **不允许 force-overwrite vault.db**(`cp -f` / `mv -f` 旧文件覆盖)— SQLite WAL 日志半同步状态会损
- ❌ **不允许并发跑 N 个 attune 进程**升级(单实例锁,WAL 写竞争)
- ❌ **不允许跨 minor 跳级**(不能 v1.0.0 直接到 v1.2.0)— 必须 v1.0.0 → v1.0.x → v1.1.x → v1.2.0 链式升,
  每个 minor 跑一次 migration

**Vault 永远不离设备**:升级流程仅本地操作,vault.db 不上传 cloud。

---

## 1. Pre-flight check (升级前自检)

任何升级路径之前必须满足:

| # | 检查项 | 命令 |
|---|--------|------|
| 1.1 | 当前版本已知 | `attune --version`(应返 `v1.0.0` 等) |
| 1.2 | 目标版本已下载/已可访问 | GitHub release page 看到新 tag |
| 1.3 | 磁盘 ≥ 500MB 空闲(backup + new install) | `df -h ~/` |
| 1.4 | vault unlock 状态(避免升级中迁移要 password) | `attune status` 返 `unlocked` |
| 1.5 | **pre-upgrade backup 已生成** | `attune pre-upgrade-backup`(详 §1.6) |

### 1.6 Pre-upgrade backup

```bash
# v1.0.1+ 提供:
attune pre-upgrade-backup

# 输出:
# ✓ Backup created: ~/.local/share/Attune/backups/vault.db.bak.20260525-1430
# ✓ SHA256: a1b2c3...
# ✓ Size: 12.3 MB
```

backup 文件 retention:**自动保留最近 5 份**;更早的自动淘汰。手动清理:
```bash
ls -lht ~/.local/share/Attune/backups/
rm ~/.local/share/Attune/backups/vault.db.bak.<old-timestamp>
```

---

## 2. Desktop App (Tauri auto-updater)

**默认路径**(99% 用户走这条):

### 2.1 自动检测

Tauri auto-updater 每 4 小时 silent check `https://github.com/qiurui144/attune/releases/latest/download/latest.json`。
新版本可用时,顶栏弹出更新通知(可关闭 / 暂缓)。

### 2.2 手动检查

UI Settings → About → 点击 "Check for updates" 按钮。

### 2.3 应用更新

点击 "Apply update" → 流程:

1. **后台下载 .deb / .exe / .AppImage**(根据 OS)
2. **校验 minisign 签名**(`tauri.conf.json::pubkey`)
3. **prompt 用户确认 + 自动 pre-upgrade backup**(per §1.6)
4. **restart attune** 应用新版本
5. **首次启动跑 vault migration**(若 schema_version bump)
6. **健康检查**:UI 正常起,vault 可解锁,chat history 可见 = 升级成功

### 2.4 升级失败 fallback

- minisign 签名校验 fail → 立即终止 + UI 错误提示 + 不动 vault → **不会丢数据**
- migration fail → 启动器自动 rollback 到 backup → 提示用户走 §7 Disaster Recovery
- 网络断开 → 下次 silent check 继续

### 2.5 手动重装路径(auto-updater 不可用时)

1. 跑 `attune pre-upgrade-backup`(per §1.6)
2. 从 release page 下载 `Attune_<version>_<platform>.{deb,msi,AppImage}`
3. `sudo dpkg -i Attune_*.deb` / 双击 .msi / `chmod +x AppImage && ./AppImage`
4. 重启 attune,验 §2.3 第 6 步健康检查

---

## 3. CLI / Server tarball (高阶用户)

### 3.1 适用场景

- Linux 服务器 headless 部署(无 GUI)
- 自动化升级管道 / docker / k8s
- K3 一体机(走镜像 reflash,非 .deb,详 §3.4)

### 3.2 标准升级

```bash
# 1. backup
attune pre-upgrade-backup

# 2. stop old server
systemctl --user stop attune-server || pkill attune-server-headless

# 3. download new tarball
curl -LO https://github.com/qiurui144/attune/releases/download/v1.0.1/attune-v1.0.1-x86_64-linux-gnu.tar.gz

# 4. verify SHA256
sha256sum -c attune-v1.0.1-x86_64-linux-gnu.tar.gz.sha256

# 5. extract + replace
tar -xzf attune-v1.0.1-x86_64-linux-gnu.tar.gz
sudo cp attune-v1.0.1/bin/* /usr/local/bin/

# 6. verify
attune --version  # 应返 v1.0.1

# 7. restart server
systemctl --user start attune-server
```

### 3.3 systemd 用户 service

`~/.config/systemd/user/attune-server.service` 不需要改 — 二进制路径一致。
`systemctl --user daemon-reload` 不必跑(unit 文件未改)。

### 3.4 K3 一体机镜像化路径(RVA23)

K3 一体机**不走** .deb / tarball,走镜像化部署:

```bash
# 1. user 把 vault.db 主动 export 到 USB / SD card
attune vault-export /mnt/usb/vault-export.bin

# 2. K3 reflash 新镜像(rv-baseos + attune v1.0.1 预装)
# 走 K3 厂商提供的 reflash 工具(参 docs/k3-ai-service/)

# 3. 启动新镜像,import vault.db
attune vault-import /mnt/usb/vault-export.bin
```

**K3 user 注意**:K3 form factor 不参与 Tauri auto-updater(详见 §8 Known Limitations)。

---

## 4. Cloud SaaS (maintainer 内部 ops)

> 本节面向 cloud 部署 maintainer(qiurui144 一人),普通 user 不涉及。

### 4.1 Zero-downtime 升级 path(v1.0.2+)

v1.0.1 **尚未实施蓝绿**(per spec §2 推 v1.0.2)。当前为 **maintenance window** 升级:

```bash
# 1. announce 维护(blog post + status page)
# 2. ssh cloud
ssh attune-cloud
cd /opt/cloud

# 3. backup DB
docker compose exec -T postgres pg_dump -U attune attune > /tmp/attune-$(date +%Y%m%d-%H%M).sql

# 4. backup volumes
tar -czf /tmp/cloud-volumes-$(date +%Y%m%d-%H%M).tar.gz /var/lib/docker/volumes/

# 5. pull new image
docker compose pull

# 6. apply DB migration(若有)
docker compose run --rm accounts python manage.py migrate

# 7. restart all services
docker compose up -d --remove-orphans

# 8. health check
curl https://accounts.attune.example.com/health
curl https://wiki.attune.example.com/health
curl https://gateway.attune.example.com/v1/models
```

### 4.2 DB migration 顺序

如果 cloud minor 升级带 DB schema 变更,migration 顺序:

1. **accounts**(用户表 / quota / DSAR)
2. **gateway**(token usage log)
3. **wiki**(文档表)
4. **monitoring-stack**(Prometheus 配置,无 schema)

每步跑完 verify endpoint 200 才进下一步。

### 4.3 Rollback path

```bash
# 1. stop new
docker compose down

# 2. revert image tag(docker-compose.yml `image: ghcr.io/qiurui144/cloud-accounts:v2.2.0`)
git checkout HEAD~1 docker-compose.yml

# 3. restore DB
docker compose up -d postgres
cat /tmp/attune-<ts>.sql | docker compose exec -T postgres psql -U attune attune

# 4. start old
docker compose up -d

# 5. verify
curl https://accounts.attune.example.com/health
```

---

## 5. attune-pro Plugin Pack

### 5.1 默认路径(Settings UI)

1. attune UI → Settings → Plugins → "Check for updates"
2. attune-pro 显示 "v1.0.1 available" → click Install
3. attune 后台 `attune plugin-install <pkg>` 自动跑

### 5.2 CLI 路径

```bash
# 1. uninstall 老版本
attune plugin-uninstall attune-pro

# 2. download 新版本 .atplugin
curl -LO https://github.com/qiurui144/attune-pro/releases/download/v1.0.1/attune-pro-v1.0.1.atplugin

# 3. verify minisign 签名
attune plugin-verify-sig --plugin-dir attune-pro-v1.0.1.atplugin

# 4. install
attune plugin-install attune-pro-v1.0.1.atplugin

# 5. verify
attune plugin-list  # 应见 attune-pro v1.0.1
```

### 5.3 Plugin 升级失败 rollback

```bash
# 1. uninstall failed plugin
attune plugin-uninstall attune-pro

# 2. install previous version(若已 cache)
attune plugin-install ~/.cache/attune/plugins/attune-pro-v1.0.0.atplugin

# 3. 若 cache 已无,从 release page download v1.0.0 重装
```

---

## 6. v0.7 → v1.0 老用户特殊路径

v0.7.x → v1.0.0 跨 13 个 minor,**必须**走中间分步升级:

| 阶段 | 跳板 | 关键 migration |
|------|------|---------------|
| v0.7.x | v0.8.0 | items.summary 表新增,自动 backfill |
| v0.8.x | v0.9.0 | 4 new law-pro agent table |
| v0.9.x | v1.0.0 | schema_version 字段引入,vault.db magic header 变更 |
| v1.0.x | latest | patch only(无 schema 变更) |

**实施**:

```bash
# 0. backup(必跑)
attune pre-upgrade-backup
cp ~/.local/share/Attune/vault.db ~/.local/share/Attune/vault.db.v0.7.0-pristine
# (额外 cold backup 到 USB 也建议)

# 1. install v0.8.0(release page 历史 tag 下载)
curl -LO https://github.com/qiurui144/attune/releases/download/v0.8.0/Attune_0.8.0_amd64.deb
sudo dpkg -i Attune_0.8.0_amd64.deb
attune status  # verify migration 完成

# 2-3. 重复 v0.9.0 / v1.0.0
# ...

# 4. 最后 v1.0.0 → v1.0.1 走 §2 常规 path
```

**禁止**:v0.7.x 直接装 v1.0.0 — 跨多个 schema 不兼容,migration 会 fail。

---

## 7. Disaster Recovery 链入口

升级失败后:

1. **第一步**:不要慌,vault.db 已有 backup(per §1.6)
2. **第二步**:跑 `attune rollback`(详见 [`docs/ROLLBACK.md`](ROLLBACK.md))
3. **第三步**:若 rollback 失败 → 走 ROLLBACK.md §3 "vault.db 从 backup 手动 restore"
4. **第四步**:若 backup 也损 → 走 ROLLBACK.md §6 "导出 plain-text 应急 export"

详见 [`docs/ROLLBACK.md`](ROLLBACK.md) 完整 playbook。

---

## 8. 已知风险 + Known Limitations

### 8.1 K3 一体机不走 Tauri auto-updater

K3 form factor 用 RVA23 riscv64 + 镜像化部署,不参与 .deb / .AppImage 流水线。
K3 升级走厂商提供的 reflash 工具(详 §3.4)。

**推 v1.0.7**:K3 镜像 OTA workflow(基于 mtd 分区切换)。

### 8.2 macOS / aarch64 暂不支持

per `CLAUDE.md` "macOS 暂不做" + "Linux aarch64 v1.x 不投入"。

### 8.3 minisign 私钥 rotation playbook

v1.0.1 走单一 minisign keypair,无 rotation 流程。**推 v1.0.4**:keypair rotation
+ revoke 旧 signature + force re-sign 历史 release。

### 8.4 SLA 数值化

P0/P1/P2/P3 turnaround time 暂未数值化(detection time / response time / resolution time)。
**推 v1.0.2**:SLA spec + status page。临时入口 `docs/SUPPORT.md`(v1.0.1 占位 placeholder)。

### 8.5 Plugin auto-update

attune-pro plugin 当前为半手动(UI 提示 + click install)。**推 v1.0.10**:全自动 plugin
silent update 流水线(与 Tauri updater 同 latest.json 机制对齐)。

---

> 维护者:本文档随每个 minor 增量补充新节,**禁止**新建 `UPGRADING-v1.0.x.md` 单独文件(per § 文档体系铁律)。
