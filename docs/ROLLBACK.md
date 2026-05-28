# Attune Disaster Recovery + Rollback Playbook (ROLLBACK.md)

> **SSOT 范围**:升级失败 / vault 损坏 / 服务异常 5 类不可逆场景的 user 恢复路径。
> 升级正向路径见 [`docs/UPGRADING.md`](UPGRADING.md)。

## 目录 (Table of Contents)

- [0. 适用范围 + 触发场景](#0-适用范围--触发场景)
- [1. 启动器自动 rollback (Tauri 内置)](#1-启动器自动-rollback-tauri-内置)
- [2. CLI `attune rollback` 子命令](#2-cli-attune-rollback-子命令)
- [3. Vault.db 从 backup 手动 restore](#3-vaultdb-从-backup-手动-restore)
- [4. Cloud gateway 改坏 (maintainer)](#4-cloud-gateway-改坏-maintainer)
- [5. DB ALTER 失败 (pg_restore)](#5-db-alter-失败-pg_restore)
- [6. Plugin pack 升级后异常](#6-plugin-pack-升级后异常)
- [7. attune-pro signing key 泄露](#7-attune-pro-signing-key-泄露)
- [8. 应急 plain-text export](#8-应急-plain-text-export)
- [9. K3 一体机离线 rollback](#9-k3-一体机离线-rollback)
- [10. Exit code 速查](#10-exit-code-速查)

---

## 0. 适用范围 + 触发场景

| 场景 | 严重度 | 章节 |
|------|--------|------|
| 升级后启动 panic / 黑屏 | 🔴 critical | §1, §2 |
| vault.db 半同步 / corrupt | 🔴 critical | §3 |
| cloud accounts 容器起不来 | 🔴 critical | §4 |
| postgres ALTER 中断 | 🟡 high | §5 |
| attune-pro plugin 启动后崩 | 🟡 high | §6 |
| minisign 私钥泄露 | 🔴 critical | §7 |
| backup 也损,要紧急导数据 | 🔴 critical | §8 |
| K3 一体机镜像启动失败 | 🟡 high | §9 |

**通用红线**:

- ❌ rollback 过程中不允许 force-delete `~/.local/share/Attune/`(用户数据)
- ❌ 不允许在 vault unlocked 状态下 cp / mv vault.db(WAL 写竞争损数据)
- ❌ 不允许跨 minor 跳级 rollback(同 UPGRADING.md §0)

---

## 1. 启动器自动 rollback (Tauri 内置)

**触发**:Tauri auto-updater 装新版后,启动器首次 healthcheck fail(进程 30s 内 SIGCRASH)。

**自动流程**(v1.0.1+ 启动器内置):

1. 启动器检测进程异常退出(exit code ≠ 0,30s 内)
2. 自动从 `~/.local/share/Attune/backups/vault.db.bak.<最新>` restore
3. 自动 downgrade 到上一个安装包(从 `/var/cache/apt/archives/attune_v*.deb` 或 `~/.cache/attune/installers/`)
4. 通知用户:"已自动 rollback 到 v<old>。问题已上报。"

**user 不需要任何操作**。若仍异常 → 走 §2。

---

## 2. CLI `attune rollback` 子命令

### 2.1 列出可用 backup

```bash
attune rollback --list

# 输出示例:
# Available backups (newest first):
#   0: vault.db.bak.20260525-1430  (12.3 MB, SHA256: a1b2c3...)
#   1: vault.db.bak.20260524-0915  (12.1 MB)
#   2: vault.db.bak.20260523-1822  (11.9 MB)
#   3: vault.db.bak.20260522-1006  (11.7 MB)
#   4: vault.db.bak.20260521-1545  (11.5 MB)
```

### 2.2 Restore 最新 backup

```bash
attune rollback

# 等同于 attune rollback --index 0
# 自动:
#   1. lock vault(避免写竞争)
#   2. SHA256 验 backup 文件
#   3. cp ~/.local/share/Attune/backups/vault.db.bak.<latest> → ~/.local/share/Attune/vault.db
#   4. unlock vault
#   5. verify item_count >= backup recorded
```

### 2.3 Restore 指定 backup

```bash
attune rollback --index 2  # 用第 3 老的(2026-05-23 那份)
```

### 2.4 Rollback 失败 exit code

| code | meaning |
|------|---------|
| 0 | 成功 |
| 11 | no backup found(`~/.local/share/Attune/backups/` 为空) |
| 12 | disk space < 100MB(无法 cp) |
| 13 | SHA256 mismatch(backup 损坏)→ 走 §3 |
| 14 | --index 超出范围 |
| 15 | vault 仍被另一进程持锁(stop attune-server) |

---

## 3. Vault.db 从 backup 手动 restore

**触发**:§2 `attune rollback` 也 fail(exit 13 / 15)。

**纯文件系统操作**(适用 attune 进程完全不可启动场景):

```bash
# 1. 确保 attune 全停
pkill -9 attune
pkill -9 attune-server-headless
ps aux | grep attune  # 应无残留进程

# 2. 找最新 backup
ls -lht ~/.local/share/Attune/backups/

# 3. 验 SHA256
cd ~/.local/share/Attune/backups/
sha256sum -c vault.db.bak.20260525-1430.sha256

# 4. 备份当前(损坏的)vault
mv ~/.local/share/Attune/vault.db ~/.local/share/Attune/vault.db.broken-$(date +%s)
mv ~/.local/share/Attune/vault.db-wal ~/.local/share/Attune/vault.db-wal.broken-$(date +%s) 2>/dev/null
mv ~/.local/share/Attune/vault.db-shm ~/.local/share/Attune/vault.db-shm.broken-$(date +%s) 2>/dev/null

# 5. cp backup → 主路径
cp ~/.local/share/Attune/backups/vault.db.bak.20260525-1430 ~/.local/share/Attune/vault.db

# 6. 启 attune,验
attune status
# 应输出 "unlocked" 或 "locked"(取决于上次状态),且 item_count > 0
```

**注**:`vault.db-wal` / `vault.db-shm` 是 SQLite WAL 模式辅助文件,重启后会自动重建,**故意删掉**即可。

---

## 4. Cloud gateway 改坏 (maintainer)

> 仅 cloud maintainer 涉及,普通 user 跳过。

**触发**:`docker compose up -d` 新 image 后,`curl gateway/v1/models` 返 500 / 502。

```bash
ssh attune-cloud
cd /opt/cloud

# 1. 看 log 找 root cause
docker compose logs --tail 100 gateway

# 2. revert image tag(临时)
git checkout HEAD~1 docker-compose.yml

# 3. restart
docker compose up -d gateway

# 4. verify
curl https://gateway.attune.example.com/v1/models
# 应返 200 + models list

# 5. 若仍 fail → 走 §5 DB 层 rollback
```

---

## 5. DB ALTER 失败 (pg_restore)

**触发**:docker compose 跑 alembic migration 中断(host SIGTERM / disk full)。

```bash
ssh attune-cloud
cd /opt/cloud

# 1. stop accounts(写入端)
docker compose stop accounts

# 2. 从最近 dump 恢复
ls -lht /backups/attune-*.sql | head -1
docker compose exec -T postgres psql -U attune -c "DROP DATABASE attune;"
docker compose exec -T postgres psql -U attune -c "CREATE DATABASE attune;"
cat /backups/attune-<latest>.sql | docker compose exec -T postgres psql -U attune attune

# 3. revert accounts image tag(若与新 schema 绑定)
git checkout HEAD~1 docker-compose.yml

# 4. start
docker compose up -d accounts

# 5. verify
curl https://accounts.attune.example.com/health
docker compose exec accounts python manage.py showmigrations
# 应见 migration 状态回到 dump 那一刻
```

---

## 6. Plugin pack 升级后异常

**触发**:`attune plugin-install attune-pro-v1.0.1.atplugin` 后,attune 启动 panic 或 attune-pro feature 全不可用。

```bash
# 1. uninstall 失败版
attune plugin-uninstall attune-pro

# 2. 列 cache
ls ~/.cache/attune/plugins/

# 3. install 上一版
attune plugin-install ~/.cache/attune/plugins/attune-pro-v1.0.0.atplugin

# 4. verify
attune plugin-list
# 应见 attune-pro v1.0.0

# 5. restart attune,验律师 feature 可用
```

**若 cache 已无**:从 release page 历史 tag `https://github.com/qiurui144/attune-pro/releases/tag/v1.0.0` 下载。

---

## 7. attune-pro signing key 泄露

**严重度**:🔴 critical — 任何持泄露 key 的攻击者可冒签恶意 plugin。

**maintainer 应对流程**(qiurui144 一人):

1. **立即 revoke 旧 pubkey**:
   - attune-pro release page 发公告 "Compromise notice: pubkey rotated"
   - `attune-pro/PUBKEY` 文件改新 pubkey + git tag `pubkey-rotation-<date>`
2. **生成新 keypair**(`minisign -G -W -p /etc/attune-pro/new-pubkey.pub -s /etc/attune-pro/new-priv.key`)
3. **force re-sign 历史 release**:跑 `scripts/re-sign-all-releases.sh`(每个 .atplugin 用新 priv key 重签)
4. **attune 客户端走 v1.0.4 keypair rotation flow**(per UPGRADING.md §8.3):
   - 客户端 fetch `https://attune.example.com/pubkey-revocation.json`
   - 检测 fingerprint 在 revocation list → 强制升级 attune-pro plugin
5. **SECURITY.md incident** 记 post-mortem(根因 / scope / 缓解)
6. user 不需要操作(自动 silent 滚到新 pubkey)

**预防**:keypair 存 `/etc/attune-pro/priv.key` chmod 600,trufflehog pre-commit。

---

## 8. 应急 plain-text export

**触发**:§3 也 fail(backup 也损),需紧急救出用户数据。

**前提**:vault.db 文件**部分可读**(非全 0)。

```bash
# 1. install 最新 attune CLI(headless 模式)
sudo dpkg -i Attune_1.0.1_amd64.deb

# 2. 尝试 unlock
attune unlock
# 输入 master password

# 3. 紧急 export(纯 JSON,不加密)
attune vault-export ~/attune-emergency-$(date +%Y%m%d).json

# 4. 若 unlock fail(KDF 损)→ raw SQLite query 抢救
sqlite3 ~/.local/share/Attune/vault.db ".dump" > ~/attune-raw-$(date +%Y%m%d).sql
# 警告:内容仍是 AES-256-GCM 密文,需 master password + device secret 才能解密
```

**注意**:emergency export 是 **plain-text 明文**,保存到 USB / 加密目录,**不要**上传 cloud / 邮件。

---

## 9. K3 一体机离线 rollback

**触发**:K3 reflash 新镜像后开机 fail(rv-baseos panic / attune service 起不来)。

**流程**(完全离线,无 GitHub 依赖):

1. **拔 K3 电源**,SD card 插宿主机
2. **dd 老镜像回写**:
   ```bash
   # 老镜像位于 ~/.cache/attune/k3-images/attune-k3-v1.0.0.img
   sudo dd if=~/.cache/attune/k3-images/attune-k3-v1.0.0.img of=/dev/sdX bs=4M status=progress
   sudo sync
   ```
3. **SD card 插回 K3** → 启 → 应见 v1.0.0 旧 UI
4. **若老镜像也无 cache**:走 厂商提供的 recovery tool(参 `docs/k3-ai-service/`)

**预防**:K3 用户**每次升级前**先 export vault 到 USB,镜像化升级**默认有数据丢失风险**(per UPGRADING.md §3.4)。

---

## 10. Exit code 速查

| code | scenario | recovery |
|------|----------|----------|
| 0 | success | — |
| 11 | no backup found | 走 §3 手动 restore;若仍 fail 走 §8 |
| 12 | disk full | 清盘后重试;`df -h ~` |
| 13 | SHA256 mismatch | backup 损,走 §3 cp 损坏文件 + §8 emergency export |
| 14 | --index out of range | `attune rollback --list` 看可用 index |
| 15 | vault locked by other process | `pkill attune` 后重试 |

---

> 维护者:任何 rollback 路径**禁止**rm `~/.local/share/Attune/`(用户数据)。出错时 cp 损坏文件
> 到 `<name>.broken-<timestamp>` 保留,**永远不 force delete**。
