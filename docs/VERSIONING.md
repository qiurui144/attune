# Attune Ecosystem Versioning Policy

> **SSOT** for tags, releases, and changelogs across the **four-repo attune ecosystem**.
> All sibling repos (attune-pro / attune-pluginhub / cloud) reference this document.
> Established 2026-05-20.

## 1. Repo / 版本线一览

| 仓 | 路径 | 版本线 | tag 形态 | RELEASE.md 位置 |
|----|------|--------|---------|----------------|
| **attune** (OSS) | `/data/company/project/attune` | SemVer `v0.X.Y` | `v<X.Y.Z>` + `desktop-v<X.Y.Z>` (双轨) | `RELEASE.md` (Python) + `rust/RELEASE.md` (Rust 商用线) + `CHANGELOG.md` (项目级) |
| **attune-pro** (private) | `/data/company/project/attune-pro` | **配对 attune 同号** | `v<X.Y.Z>`(整仓快照) + `<pack-id>/v<X.Y.Z>`(单包独立发版) | `RELEASE.md` |
| **attune-pluginhub** | `/data/company/project/attune-pluginhub` | 平台自身 SemVer + 包级 tag | `v<X.Y.Z>`(平台) + `<pack-id>/v<X.Y.Z>`(发布到 hub 的包) | `RELEASE.md` |
| **cloud** | `/data/company/cloud` | 独立 SemVer `v2.X.Y` | `cloud-v<X.Y.Z>`(meta) + 服务内 tag(可选,如 `accounts-v1.1.0`) + `deploy/YYYY-MM-DD-HHMM`(部署快照) | `RELEASE.md` |

## 2. Tag 注解必须包含(强制)

每个 annotated tag 的 message 必须有:
1. **版本号**(标题行)
2. **关联版本**(同周期或配对 attune 版本)
3. **关键变更摘要**(3-6 行,从 RELEASE.md 当节剪裁)
4. **commit count**(可选,大版本必填)

**反模式**:`v0.7.0` 一行裸 tag 不可接受。`git tag -a v0.7.0 -m "v0.7.0"` 不合规。

**模板**:
```
v0.7.0 — <主题一行>

attune v0.7.0 (paired)
2026-05-19

主要变更:
- <bullet 1>
- <bullet 2>
- ...

详见 RELEASE.md
```

## 3. RELEASE.md 是 SSOT(强制)

- 每个仓必须有 `RELEASE.md`(或 `rust/RELEASE.md` 兼容已有结构)
- **tag annotation + GitHub Release body 都从 RELEASE.md 引用**,**禁止有独立内容**
- 新版本节添加在文档**顶部**(倒序时间),不在底部追加
- 每个版本节包含:**日期 / 版本号 / 一句话 highlight / 分类变更列表 / 升级须知(若有 break change)**

## 4. GitHub Release 是公开面(发版型 tag 必有)

仅适用 **GA / patch release** 类型的 tag:
- 必须在 GitHub 上有对应 Release
- Release body 引用 RELEASE.md 当节
- 必须挂 release-workflow 生成的 artifacts (二进制 tarball / installer / .deb 等)
- alpha / beta / rc 类型可以**不开** GitHub Release(tag 即可)

**治理对齐型 merge**(不发版的 develop → main merge)**不打 tag** —— 见 `CLAUDE.md` 的 GitFlow Lite 节。

## 5. 跨仓配对规则

### 5.1 attune × attune-pro(强配对)

- attune 每发一个 `v<X.Y.Z>`,attune-pro **同一天同号**打 tag(整仓快照)
- 语义:"这是 attune v<X.Y.Z> 用户买 attune-pro 时拿到的 plugin pack 集合"
- attune-pro tag 注解必须含:`attune v<X.Y.Z> (paired)` + 该周期内含的 pack 版本清单

### 5.2 attune-pro 包级 tag(独立发版)

- 单个 pack(如 law-pro)需要 out-of-band hotfix → 打 `law-pro/v<X.Y.Z>`
- 这种 tag 不影响 attune-pro 整仓版本号
- pluginhub 上传该 hotfix 包后必须在 pluginhub 也打对应 `<pack>/v<X.Y.Z>` tag

### 5.3 cloud(独立版本)

- cloud 有自己的 v2.x.x 线,不与 attune 配对
- cloud RELEASE.md 当节必须注明:**支持的 attune 客户端版本范围**(如 `compatible with attune v0.6.x–v0.7.x`)
- 日常部署用 `deploy/YYYY-MM-DD-HHMM` 快照 tag,正式发版用 `cloud-v<X.Y.Z>`

### 5.4 pluginhub

- 平台升级用 `v<X.Y.Z>`(46 行 RELEASE.md 已有 v1.0.0 基线)
- 每次有 plugin pack 上传到 hub 同步打 `<pack-id>/v<X.Y.Z>` tag,锚定该包在 hub 的对应物

## 6. 发版前 checklist(每次正式 tag 前必跑)

```
□ RELEASE.md 顶部已添加本版本节,日期 / 变更分类 / 升级须知齐全
□ 兄弟仓需配对的也已准备(attune ↔ attune-pro)
□ cargo test / pytest 全绿(适用仓)
□ cargo clippy -D warnings 零警告(Rust 仓)
□ docs/VERSIONING.md 若有规则变更同步更新
□ scripts/version-audit.sh 跑过,无 cross-repo 漂移
□ 工作目录干净(git status 无 modified)
□ 当前在 main(发 GA)或 develop(发 alpha/beta/rc)分支
```

## 7. 发版后 verification(每次 tag push 后必跑)

```
□ git push origin <tag> 成功
□ GitHub Actions release workflow 触发并跑通(适用 attune)
□ GitHub Release 页面已建并挂载 artifacts(GA 版必查)
□ tag annotation message 正确显示(gh release view <tag>)
□ 兄弟仓同步 tag 已 push(配对版本)
□ scripts/version-audit.sh 跑过,新 tag 已被发现
```

## 8. 周审计(每周一次)

`scripts/version-audit.sh` 验证:
- 各仓 tag 总数 / annotated 比例(应 100% annotated)
- main HEAD 与最新 tag 是否对齐
- attune ↔ attune-pro 版本配对是否同步
- RELEASE.md 是否覆盖到最新 tag
- 任何 lightweight tag 立即标红

CI 上 weekly cron 跑(规划中,目前手工跑)。

## 9. Backfill 历史(2026-05-20 第一次回填)

| 仓 | 状态 | 行动 |
|----|------|------|
| attune | 30 annotated tag,健康 | 无需 backfill |
| attune-pro | 0 tag → v0.7.0 backfill @ `aab60f4` | ✅ 完成 2026-05-20 |
| cloud | 0 tag → cloud-v2.1.0 backfill @ `4838061` | ✅ 完成 2026-05-20 |
| pluginhub | 0 tag,有 RELEASE.md v1.0.0 节 → backfill v1.0.0 | 待执行(下次有 plugin 上传时一并打) |

历史发版若需追加 tag,在原版本节同步标注 "backfilled @ <date>"。

## 10. 反模式(违反即拒绝)

- ❌ lightweight tag(`git tag v0.7.0` 不加 `-a/-s`)
- ❌ tag annotation 一行裸名(`-m "v0.7.0"` 无内容)
- ❌ RELEASE.md 没更新就打 tag
- ❌ attune 打了 v0.7.0 但 attune-pro 没同步打
- ❌ GA tag 没有对应 GitHub Release(对外用户找不到 release notes)
- ❌ 把 alpha / beta / rc tag 当 GA 推荐给用户(`latest` release 必须是 GA)
- ❌ 删除已 push 的 tag(除非用户明确同意 + 远端无引用)
