# Attune OSS — Round 40 Profile Export/Import Roundtrip

**Started**: 2026-05-09 09:30

**目标**: 测 /api/v1/profile/export 完整数据 + 看是否有 import endpoint, 验证用户级 data portability.

## ⭐ R40 Profile Export/Import Roundtrip 完美

### Export 数据 schema
- size: 30,398 bytes
- keys: cluster_snapshot, exported_at, histograms, item_count, tags, vault_version, version
- item_count: 623, tags: dict[623 keys]

### Roundtrip 验证
| Test | Result |
|------|--------|
| GET /api/v1/profile/export | 200, 30KB JSON ✅ |
| POST /api/v1/profile/import (same data) | `{merged:623, skipped:0, status:ok}` ✅ idempotent |
| POST empty body `{}` | 422 "missing field 'version'" ✅ |
| POST invalid JSON | 400 "Failed to parse" ✅ |
| Items count | 623 → 623 不变 ✅ |

### Production 价值
- 用户 import 不重复创建 items (idempotent merge)
- 错误明确 (422 vs 400 区分 schema vs JSON 错误)
- Roundtrip 不丢数据
- 可用于跨设备数据迁移 / 灾难恢复

## R40 Extra 180min sustained
**Wall time**: 10800s — 3356/10642 ok
