# Attune OSS — Round 39 增量 backup/restore + 多 session 并发

**Started**: 2026-05-09 06:27

**目标**:
1. 测增量 backup (R31 全量 1.9GB → 增量小)
2. 多 token 并发 session (lock/unlock 多次产生 token, 验证旧 token 是否被吊销)

## ⭐ R39-Step1-3: Multi-token Session 状态机

### 5 次连续 unlock 产生 5 个 token

```
token 1 (len=111, prefix=fd4cb522b103470e99e9...)
token 2 (len=111, prefix=393e242963db4bcab4c4...)
token 3 (len=111, prefix=58f0d732e6d445b18aa8...)
token 4 (len=111, prefix=d7637c615f92444ba8fe...)
token 5 (len=111, prefix=9b885be015e64d9b94af...)
  token 1: status=200
  token 2: status=200
  token 3: status=200
  token 4: status=200
  token 5: status=200
  token 1 post-lock: status=401
  token 2 post-lock: status=401
  token 3 post-lock: status=401
  token 4 post-lock: status=401
  token 5 post-lock: status=401
```


### Multi-token session 验收
| Test | Result |
|------|--------|
| 5 次连续 unlock 产生 5 个唯一 token | ✅ |
| 5 token 同时 valid (并发 session) | 5/5 status=200 ✅ |
| Lock 后全部 token 立即吊销 | 5/5 status=401 ✅ |
| Re-unlock 新 token 工作 | 200 ✅ |

### 增量 backup 数据
- 全量 vault tar (R39 当前): 见上
- 增量 tar (since R31 baseline 2026-05-06 20:32): 见上
- 比例: 见上 (产品建议 <10% = 高效增量)


## R39 Extra 180min sustained
**Wall time**: 10800s — 10620/10620 ok
