# Attune Manual Test Checklist

人工验收清单。每次发布前由测试人员逐项勾选。打 `[x]` 为通过，`[-]` 为跳过（注明原因），`[ ]` 为未测。

---

## 目录 (Table of Contents)

- [v1.0 GA Acceptance (30 items)](#v10-ga-acceptance-30-items)
  - [A. 安装 (5)](#a-安装-5)
  - [B. Wizard 首启 (6)](#b-wizard-首启-6)
  - [C. 核心功能 (8)](#c-核心功能-8)
  - [D. OCR / ASR (5)](#d-ocr--asr-5)
  - [E. Plugin / Marketplace (3)](#e-plugin--marketplace-3)
  - [F. 异常与恢复 (3)](#f-异常与恢复-3)
- [v0.6 GA Acceptance (legacy reference)](#v06-ga-acceptance-legacy-reference)

---

## v1.0 GA Acceptance (30 items)

**测试版本**：`desktop-v1.0.0-rc.2` / GA `v1.0.0`  
**测试平台**（勾选本次覆盖的）：

- [ ] Windows x86_64 — NSIS exe
- [ ] Windows x86_64 — MSI
- [ ] Linux x86_64 — deb
- [ ] Linux x86_64 — AppImage
- [ ] Linux x86_64 — rpm

**测试人**：___________  **测试日期**：___________  **备注**：___________

---

### A. 安装 (5)

| # | 项目 | 平台 | 结果 | 备注 |
|---|------|------|------|------|
| A1 | 双击 `Attune_1.0.0_x64-setup.exe` 完整安装无报错，应用出现在开始菜单 | Win | [ ] | |
| A2 | `sudo dpkg -i Attune_1.0.0_amd64.deb` 安装成功，`postinst` 执行完毕无 `E:` 错误 | Linux deb | [ ] | |
| A3 | `chmod +x *.AppImage && ./Attune_1.0.0_amd64.AppImage` 启动成功 | Linux AppImage | [ ] | |
| A4 | `sudo dnf install *.rpm` 安装成功 | Linux rpm | [ ] | |
| A5 | 重复安装（覆盖安装）不报错，用户数据保留 | Win / Linux | [ ] | |

---

### B. Wizard 首启 (6)

| # | 项目 | 预期 | 结果 | 备注 |
|---|------|------|------|------|
| B1 | 首次启动自动进入 Wizard Step 1（欢迎页） | 显示欢迎页，有"开始设置"按钮 | [ ] | |
| B2 | Step 2 设置主密码 ≥12 字符含字母数字，强度指示显示"强" | 弹出恢复密钥下载，密钥文件 `attune-recovery-key.txt` 可保存 | [ ] | |
| B3 | Step 2 密码 <12 字符 → 提示错误，禁止进入下一步 | 显示"密码至少需要 12 个字符" | [ ] | |
| B4 | Step 3 填写 LLM API key → 点击"测试连接"返回"连接成功" | 显示成功提示，可进入下一步 | [ ] | |
| B5 | Step 4 硬件检测完成，推荐 embedding 模型符合当前 RAM/GPU 配置 | 推荐与 CLAUDE.md 矩阵一致（≥16 GB + 独显 → bge-m3，等） | [ ] | |
| B6 | Step 5 绑定文件夹 → 选择本地目录 → 点击"完成·进入 Attune"进入主 UI | 主 UI 正常打开，Sources 标签可见绑定目录 | [ ] | |

---

### C. 核心功能 (8)

| # | 项目 | 预期 | 结果 | 备注 |
|---|------|------|------|------|
| C1 | 上传 PDF 文件 | Sources 列表出现文件，状态从"处理中"变为"已索引" | [ ] | |
| C2 | Chat — 输入"总结这篇文件" | 回答中引用上传文件的段落，底部显示引用来源 | [ ] | |
| C3 | Search — 输入文件中的关键词 | 返回包含该词的 chunk 列表，带来源文件名 | [ ] | |
| C4 | 锁定 Vault（顶栏锁定按钮） | 界面变为密码输入屏，所有内容不可见 | [ ] | |
| C5 | Vault 解锁（输入正确密码） | 恢复到锁定前状态，数据完整 | [ ] | |
| C6 | 重启应用后重新解锁 | 数据（文件、聊天记录）完整保留 | [ ] | |
| C7 | Settings → AI 大脑 → 更换 LLM provider 并重新测试 | 切换后 Chat 使用新 provider 正常响应 | [ ] | |
| C8 | 上传第二个 PDF → Chat 中询问跨文件问题 | RAG 正确引用两个文件的内容 | [ ] | |

---

### D. OCR / ASR (5)

| # | 项目 | 预期 | 结果 | 备注 |
|---|------|------|------|------|
| D1 | Office 标签 → 上传发票/收据图片（scene: receipt） | 返回含 vendor / amount / date 字段的结构化 JSON | [ ] | |
| D2 | Office 标签 → 上传名片图片（scene: business_card） | 返回含 name / phone / email 字段 | [ ] | |
| D3 | Office 标签 → 上传表格图片（scene: table） | 返回表格数据结构，行列关系正确 | [ ] | |
| D4 | Office 标签 → 上传音频文件（≤5 min） | WS 进度条显示处理进度，输出带时间戳文字稿 | [ ] | |
| D5 | CLI：`attune ocr <image> --profile receipt --json` | 命令行正常输出 JSON，exit code 0 | [ ] | |

---

### E. Plugin / Marketplace (3)

| # | 项目 | 预期 | 结果 | 备注 |
|---|------|------|------|------|
| E1 | 进入 Marketplace 标签 | 显示插件列表，`law-pro` 可见 | [ ] | |
| E2 | 安装 `law-pro` 插件（输入 license key 或登录 Attune Pro） | 侧边栏出现"法律助理"入口，Settings → Plugins 列表状态为 `active` | [ ] | |
| E3 | 进入 law-pro → 输入合同文本 → 点击"分析" | 返回法律要素结构化结果，无崩溃 | [ ] | |

---

### F. 异常与恢复 (3)

| # | 项目 | 预期 | 结果 | 备注 |
|---|------|------|------|------|
| F1 | Vault 锁定后输入错误密码 3 次 | 每次提示"密码错误"，不崩溃；不触发账号锁定（本地 vault 无限重试） | [ ] | |
| F2 | Chat 时断开网络（拔网线或关 WiFi） | Chat 提示"网络不可用"或 LLM 请求超时，应用不崩溃；重连后可继续 | [ ] | |
| F3 | 上传损坏/空的 PDF 文件 | 提示"文件解析失败"或忽略，Sources 列表不出现损坏条目，应用不崩溃 | [ ] | |

---

### 汇总签字

| 维度 | 通过数 / 总数 | 状态 |
|------|-------------|------|
| A — 安装 | / 5 | |
| B — Wizard | / 6 | |
| C — 核心功能 | / 8 | |
| D — OCR/ASR | / 5 | |
| E — Plugin | / 3 | |
| F — 异常 | / 3 | |
| **合计** | **/ 30** | |

**结论**：[ ] **PASS — 可发 GA** / [ ] **BLOCK — 必须修复以下项再发 GA**

Block 项：___________

测试人签字：___________ 日期：___________

---

## v0.6 GA Acceptance (legacy reference)

> v0.6 GA（2026-04-28）验收清单，存档供历史参考。

| # | 项目 | 结果 |
|---|------|------|
| L1 | .deb 安装 + Ollama 自动安装 + 拉 bge-m3 | PASS |
| L2 | NSIS exe 安装 + Wizard 完整走通 | PASS |
| L3 | PDF 上传 + 索引 + Chat RAG 引用 | PASS |
| L4 | Vault 锁定 + 解锁 + 密码错误处理 | PASS |
| L5 | Chrome 扩展连接本地 server + 侧边栏搜索 | PASS |
| L6 | Settings 更换 LLM provider | PASS |
| L7 | v0.5 → v0.6 数据升级（vault schema 幂等） | PASS |
