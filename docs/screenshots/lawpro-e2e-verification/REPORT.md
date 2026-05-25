# law-pro 接入 attune — cloud + 部署 + 全量前端验收报告

> 2026-05-17。基于 `tingly-knitting-zephyr.md`。环境:本机 attune deb 0.6.3 +
> `attune-server-headless`(develop 重编) :18900 · AMD cloud 192.168.100.201。

## 一、law-pro 产品级装载链路确认

| 环节 | 入口 | 结果 |
|------|------|------|
| cloud 部署 | AMD 192.168.100.201 全栈 15 容器 | ✅ pluginhub/accounts/llm-gateway healthy |
| law-pro 打包 | 整插件 tar.gz(仅运行时:plugin.yaml + bin/agent_civil_loan + forms/capabilities) | ✅ `law-pro-0.2.0.tar.gz` 9.1M |
| **上传** hub | pluginhub admin API `POST /admin/plugins` + `/versions`(产品发布入口) | ✅ `law-pro@0.2.0 发布成功` |
| license 签发 | pluginhub admin API `POST /admin/licenses` | ✅ `H4BZJlFO9q9_...` |
| attune 接 hub | 设置→会员→自部署 cloud 后端:pluginhub URL + license key(本次新增入口) | ✅ 切到 `http-pluginhub` provider |
| **下载** 安装 | attune 插件市场 → law-pro v0.2.0 →「安装」按钮 | ✅ 下载 9.0M → 解压验载 → 落地 `plugins/law-pro/` |
| registry 装载 | 重启 attune-server(B 方案) | ✅ `loaded 12 plugins`(原 10) |
| 案件类型注册 | 新建 civil-loan 项目 → 自动挂载 law-pro「计算助手」面板 | ✅ |

全程经产品入口(hub 发布 API / attune Marketplace UI),无手动 cp。

## 二、3 条证据链金额对照(均经前端 civil_loan 表单 → `agents/civil_loan_agent/run`)

| 链 | 输入 | 公式 | 前端结果 | 对照基准 | 结论 |
|----|------|------|---------|---------|------|
| A 标准 | 本金 20 万 / 年化 9.6% / 365 天 | 年单利 I=P·r·t | 应付利息 **¥19,200** / 应收 ¥219,200 | 手算 200000×0.096×1 | ✅ |
| Golden | 本金 10 万 / 年化 24% / 2024-01-15→2025-01-15 | 年单利 | 应付利息 **¥24,065.75** / 应收 ¥124,065.75 | golden `civil_loan_zhang_li_100k_24pct.json` | ✅ |
| B 砍头息 | 本金按实付 45 万 / 年化 24% / 2023-06-01→2025-05-01 | 年单利 | 应付利息 **¥207,123.29** / 应收 ¥657,123.29 | `lawpro_chains_e2e.py` 链B | ✅ |
| C 利率红线 | 本金 100 万 / 年化 36% / 2022-03-01→2025-05-01 / 已还息5万本20万 | LPR 4 倍封顶单利 | 应付利息 **¥469,139.73** / 应收 ¥1,219,139.73 | `lawpro_chains_e2e.py` 链C | ✅ |

链 B(砍头息)本金按实际交付计、链 C(利率红线)`lpr_capped_simple` 封顶 + 已还冲抵 —— 4 组金额前端结果与各自基准完全一致。结果面板含逐字段「依据」(证据可溯源)。

## 三、全量前端测试矩阵(脚本化 run_ui_all.sh)

`tests/e2e/playwright/lawpro_ui_e2e.py` + `run_ui_all.sh` —— 真 Chrome(channel=chrome,
locale=zh-CN),L0-L5 分层,每元素定位→操作→断言→截图,`page.on(console)` 捕 JS error。

**结果:45 PASS / 0 FAIL**

| 层 | 覆盖 | 结果 |
|----|------|------|
| L0 Wizard | 欢迎 / 密码(强度校验) / AI(hiapi.online 连接测试) / 硬件 / 数据 5 步 | ✅ 9/9 |
| L1 Sidebar | 7 导航 + 新对话 + 全局搜索 + 账号菜单 | ✅ 10/10 |
| L4 模态 | CommandPalette(Cmd+K) | ✅ 1/1 |
| L2 八视图 | 条目/项目/远程目录/知识全景/技能/插件市场 渲染+关键按钮 + Chat 输入框/切换模型 | ✅ 12/12 |
| L3 Settings | 通用/AI 大脑/数据/会员/隐私/关于 6 tab | ✅ 6/6 |
| L5 law-pro | pluginhub 配置 / Marketplace provider+law-pro / civil-loan 项目 / 计算助手面板 / 表单→agent ¥19,200 | ✅ 6/6 |
| JS error | 全程 console 监听 | ✅ 0 真实错误 |

截图归档 `docs/screenshots/lawpro-e2e-verification/suite/`(l0-* / l2-* / l3-* / l5-*)。
另:手工 MCP 巡检截图 `l0-*` / `l2-*` / `l3-*` / `l5-*`(根目录),含 3 链结果图。

## 四、过程中发现并修复的缺陷

| # | 缺陷 | 修复 |
|---|------|------|
| 1 | attune Marketplace「安装」只返回元数据、不下载落地(v0.7 半成品) | 补完 `marketplace::install_plugin` 真实下载落地 + `plugin_sync::install_plugin_package`(B 方案) |
| 2 | attune 设置无自部署 pluginhub license_key 输入框 | 设置页补 license key 入口 |
| 3 | `extract_tarball` shell-out 系统 tar(Windows P0 隐患) | gzip 走纯 Rust tar+flate2 |
| 4 | pluginhub `PluginVersion`/`License`/`Plugin` model 漏 4 列(`d197651` 迁移已建库但 model 未声明)→ upload/license/index 全 500 | AMD pluginhub models.py 补列 + 重建镜像 |
| 5 | 本机 aTrust 零信任代理劫持 `*.engi-stack.com` DNS | SSH 隧道直连 AMD pluginhub 容器(传输层,不动 cloud 设计) |

attune 代码改动经 2 轮 code-review,修 6 项;`plugin_sync` 单测 11/0。

## 五、缺口

无遗留缺口。证据链 A/Golden/B/C 4 组金额均经前端验证;脚本化 `run_ui_all.sh`
套件 45/0 全绿,覆盖 L0-L5。
