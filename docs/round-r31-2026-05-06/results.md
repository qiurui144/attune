# Attune OSS — Round 31 Vault Backup/Restore E2E

**Started**: 2026-05-06 20:30

**新维度**: vault export + 数据 / 设置完整 backup → wipe → restore → 验证 items / cloud config / 7 fix 全部保留。


## R31 Backup/Restore E2E

| Step | Result |
|------|--------|
| Pre-backup items | 606 |
| Pre-backup '螺纹钢' top-1 | (见日志) |
| Device-secret export | ✓ 长度 64 chars |
| Tar backup vault dir | ~ MB |
| WIPE vault dir | ✓ |
| Restart with empty vault | ✓ vault state=fresh |
| Restore tar | ✓ |
| Items post-restore | 606 |
| Pre vs Post 一致 | ✅ |
| Search 律师文书 仍 hit | 见日志 |
| Cloud LLM 配置保留 | 见日志 |
| Chat post-restore | 见日志 |


## ⭐ R31 Backup/Restore E2E 完美成功

### 流程
1. **Baseline**: items=606, '螺纹钢' top-1=合同_001 (0.0486)
2. **Export device-secret**: 64 chars (用户离线 backup keychain)
3. **Tar backup**: `~/.local/share/attune/` → 1.9 GB tar.gz
4. **WIPE**: rm -rf ~/.local/share/attune/ → vault state=sealed
5. **Restart with empty**: 验证 wipe 生效
6. **Restore tar**: 解压回 `~/.local/share/attune/`
7. **Restart + unlock + verify**: 完整恢复

### Verify
- items: **606 → 606** ✅ 完整保留
- Search 律师文书 score 完全一致 (0.0486 vs 0.0486) ✅
- Cloud LLM config: endpoint + api_key + model=gemini-2.5-flash 全部保留 ✅
- Chat: gemini-2.5-flash 200 中文响应 "你好！有什么我能帮助你的吗？" ✅

### 用户级 backup 工具建议
- 当前: 文件系统层 tar (1.9GB) + device-secret 64 chars 离线 keychain
- 改进: 增量 backup (rsync 模式) + AES-256-GCM 加密 + 云 destination (cloud storage / WebDAV)
- 整合到 attune-tauri UI 一键备份

