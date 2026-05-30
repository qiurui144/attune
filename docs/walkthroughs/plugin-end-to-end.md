# Plugin 端到端 walkthrough — 从开发到律师使用

完整演示一个 attune-pro plugin (`law-pro/civil_loan_agent`) 从开发者签名打包,
到律师本地装载, 到 chat 命中触发 agent, 到看到 audit_trail 的全链路.

## 前置依赖

```bash
cd /data/company/project/attune/rust
cargo build --release -p attune-cli -p attune-server -p attune-accounts

cd /data/company/project/attune-pro
cargo build --release -p law-pro --bin agent_civil_loan
```

## Phase A: Plugin 开发者侧 (打包 + 签名)

### 1. 生成签名密钥 (一次性, 离线保管)

```bash
$ attune plugin-keygen --out-priv ~/.secrets/law-pro.signing-key
✓ private key written to ~/.secrets/law-pro.signing-key (chmod 600)
PUBLIC_KEY=12fe0471d5a37735428704baa5ea7a55a937fcc490cddf5e325ef4a303e6affc
```

### 2. 加密 plugin.yaml (paid plugin)

```bash
$ ATTUNE_PLUGIN_KEY=<device-license-token> \
  attune plugin-encrypt /path/to/law-pro/
✓ encrypted to /path/to/law-pro/plugin.yaml.enc (2215 bytes)
```

### 3. 签名 plugin

```bash
$ attune plugin-sign /path/to/law-pro/ --priv-file ~/.secrets/law-pro.signing-key
✓ plugin.sig written to /path/to/law-pro/plugin.sig
  signature (base64): UrpOJ6041/e2mCRGGuXkuV9yH5Z...
```

### 4. 打包分发 (.attunepkg = tar.gz of plugin dir)

```bash
$ cd /path/to/law-pro/.. && tar czf law-pro-0.2.0.attunepkg law-pro/
```

发布到 attune-enterprise/pluginhub (attune-cloud 软链共享).

## Phase B: 律师侧 (安装 + 使用)

### 1. 下载 + 解压 .attunepkg

```bash
$ curl -O https://hub.engi-stack.com/plugins/law-pro/0.2.0.attunepkg
$ tar xzf law-pro-0.2.0.attunepkg
```

### 2. 装载到 attune (CLI 自动校验签名 + 解密)

```bash
$ ATTUNE_PLUGIN_KEY=<device-license-token> \
  attune plugin-install ./law-pro/ \
    --pubkey 12fe0471d5a37735428704baa5ea7a55a937fcc490cddf5e325ef4a303e6affc
✓ signature verified with provided pubkey → trust=Trusted
✓ parsed plugin: id=law-pro, version=0.2.0
✓ installed to ~/.local/share/attune/plugins/law-pro
Restart attune-server for the new plugin to be loaded.
```

### 3. 验证 plugin 已装

```bash
$ attune plugin-list
  law-pro (v0.2.0, type=industry, tier=paid, agents=1, skills=0, mcps=0)
  presales_pro (v0.1.0, type=industry, tier=?, ...)
  ...
7 plugin(s) installed at ~/.local/share/attune/plugins
```

### 4. 启动 attune-server (自动 scan + 装载)

```bash
$ attune-server-headless --host 127.0.0.1 --port 18900
attune-server listening on http://127.0.0.1:18900
[INFO] loaded 7 plugins, 0 workflows from ~/.local/share/attune/plugins
```

### 5. 律师 setup + unlock vault

```bash
$ attune setup
Enter master password: ***
Confirm master password: ***
Vault initialized and unlocked.

$ attune unlock
Enter master password: ***
Vault unlocked.
```

### 6. 律师上传证据到案件库

(通过 Web UI 或 API: POST /api/v1/upload + 关联 Project)

### 7. 触发 civil_loan_agent

**A. 通过 chat (路由命中)**:
```
律师输入: "梁素燕vs任其坤, 借贷本息应付多少"
↓ chat.rs match_chat_trigger → 命中 keywords=["本息", "借贷", "应付"]
→ 返回提示: "🔌 检测到此问题适合 law-pro 处理 (借贷纠纷本息合规计算)..."
```

**B. 通过 Web UI 表单 (Stage 3 律师补全)**:
```
GET /api/v1/forms/law-pro/civil_loan_stage3_form
→ ui_runtime::render_html (空字段, 律师填)
POST /api/v1/forms/law-pro/civil_loan_stage3_form/submit
→ form data 转 CivilLoanInput JSON
→ agent_runner::run_agent_subprocess
  → capability_dispatch::dispatch bin/agent_civil_loan
    → stdin: <JSON>
    → stdout: AgentOutput<CivilLoanResult> JSON
    → stderr: 【代理立场】... 【主张方向】...
→ format_agent_result_for_chat → 返回前端
```

**C. 直接 CLI (绕过 UI 测试)**:
```bash
$ echo '{"facts":{"parties":{...},"principal":500000,...},"classified_evidence":[]}' \
  | ~/.local/share/attune/plugins/law-pro/bin/agent_civil_loan

# stdout: {"computation":{"computed_interest":140000.0,...},...}
# stderr: 【代理立场】代理原告 梁素燕 ... 【主张方向】原告主张被告应付
# exit: 0 (成功) / 2 (业务红线触发, 借条不存在等)
```

## Phase C: 设备绑定 (1:2)

```bash
$ ACCOUNTS_HOST=0.0.0.0 ACCOUNTS_PORT=18901 attune-accounts
attune-accounts (reference) listening on http://0.0.0.0:18901
```

attune 客户端启动时:
1. 收集 DeviceFingerprint (device_id / hostname / form_factor)
2. POST /api/v1/devices/register {account_id, fingerprint}
3. 200 → cache DeviceLicense (30 天有效); 409 → UI 显示候选清单, 律师选踢下线

## Phase D: 红线触发示例

### 借条不存在

```bash
$ echo '{"facts":{...,"loan_doc_exists":false},...}' | agent_civil_loan
# stderr: 业务红线 ⚠️ 借条/借款合同原件不存在. 不得推定借贷关系成立...
# exit: 2
```

UI 收到 exit 2 应**不展示金额**, 弹"请补全借条原件 OR 走特殊举证路径"提示.

### 关键字段 null

```bash
$ echo '{"facts":{...,"principal":null,...},...}' | agent_civil_loan
# stderr: 业务红线 ⚠️ 本金缺失. 调用方须人工补全本金后重试.
# exit: 2
```

## 故障排查

| 现象 | 原因 | 排查 |
|------|------|------|
| install 拒 "paid/trial must be Trusted" | 未提供 --pubkey 或签名错 | `attune plugin-verify-sig <dir> --pubkey <pubkey>` 单独验证 |
| install 拒 "encrypted plugin found but no key" | paid plugin 但 ATTUNE_PLUGIN_KEY 没设 | export ATTUNE_PLUGIN_KEY=... |
| server 启动后 list 不到新装 plugin | 没重启 server | restart attune-server-headless |
| agent_civil_loan 提交后 exit 2 | 业务红线触发 (借条 null / 字段 null) | 看 stderr audit_trail 提示, UI 引导补全 |
| forms-iframe 显示 "Preflight 失败" | vault locked | attune unlock |
| forms-iframe 显示 "plugin / form 未装载" | plugin 未 install 或 form_id 错 | attune plugin-list 看是否含目标 plugin |

## 测试覆盖

| 阶段 | 测试 |
|------|------|
| Phase A 签名 | plugin_sig::sign_then_verify_with_key_succeeds (单测) |
| Phase B 装载 | attune-cli plugin-install e2e (本 walkthrough Step 2) |
| Phase B 装载 | plugin_protocol_e2e::encrypted_plugin_loads_with_correct_key |
| Phase B agent | civil_loan_agent::happy_path_outputs_audit_trail (单测) |
| Phase B agent | agent_civil_loan binary subprocess (本 walkthrough Step 7C) |
| Phase B UI | forms_routes_test::forms_endpoints_return_404_for_unknown_plugin |
| Phase B UI | playwright_forms_v2_test (preflight + 友好提示) |
| Phase C 设备 | attune_accounts::third_device_returns_409_with_existing |
| Phase C 设备 | attune_accounts::re_register_same_device_renews |
| Phase D 红线 | civil_loan_agent::red_line_no_loan_doc_returns_safe_output |
| Phase D 红线 | interest_calculator::red_line_null_principal (业务红线层) |

## 后续路线 (持续工作清单)

- [ ] **真实律师案件 demo 数据集** — 一份匿名借条 PDF + 4 笔流水截图 + 1 段微信
- [ ] **attune-server scan 支持 paid plugin 解密** — 当前装载时 .yaml.enc 不解密 (限定明文)
- [ ] **forms-iframe 用真实 form schema** — 当前路由返空 fields stub, 应从 plugin dir 读 form yaml
- [ ] **chat 命中后真 dispatch** — 当前 chat.rs 只提示, 不自动调 agent_runner (因 chat 缺 facts JSON)
- [ ] **attune-enterprise/pluginhub 集成 .attunepkg 上传 + 公钥分发** — attune-cloud 软链已就位, 缺真实分发流程
