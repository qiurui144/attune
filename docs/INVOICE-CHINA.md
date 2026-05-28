# China e-Invoice (发票) integration — planning (v1.0.8 prep)

> **状态**: planning 阶段。本文档为 v1.0.x release period 间的中国增值税电子发票合规路径
> 调研记录,**实施推 v1.0.8**(需要 user 选 supplier + 签合同 + 拿到 API 凭证)。
> v1.0.x 期间客户走临时人工开票路径(`sales@engi-stack.com`)。
>
> Per CLAUDE.md § 上云项目品质五属性「易用 / 好维护」+ § Secrets 严禁硬编码 — 发票 API 凭证
> 必须走 secrets/cloud.enc.yaml(sops+age 加密),不入代码。

## 1. 业务背景

attune 通过 Stripe 收款,Stripe 自动生成英文 receipt/invoice(`https://invoice.stripe.com/i/...`)。
中国大陆企业客户和绝大多数个人客户**报销 / 财务记账**需要中国合规的「增值税电子普通发票」
或「增值税电子专用发票」(国家税务总局 2020年第1号公告统一格式)。

Stripe 本身**不开具中国合规发票**。需要在 Stripe 收款基础上,接入中国发票服务商
(SaaS 模式),用客户填的开票信息生成合规 e-invoice 并通过邮件 / 平台下载送达。

### 1.1 合规口径(法规依据)

- **《中华人民共和国发票管理办法》**(国务院令第 587 号,2010 修订 / 2019 修订)
- **国家税务总局公告 2020 年第 1 号**:增值税电子专票全面推广
- **国家税务总局公告 2021 年第 30 号**:数电票(全电发票)试点(部分省份)
- **数据安全**:发票包含纳税人识别号 / 银行账号 / 地址 → 属 PI per PIPL §28 敏感个人信息
  → 加密存储 + 最小授权访问(per attune-pro DSAR 实施模式)

## 2. Supplier 对比(3 家国内主流)

| 维度 | 诺诺网 (Nuonuo) | 易快报 (Ekuaibao) | 百望云 (Baiwang) |
|------|-----------------|-----------------|-----------------|
| **总部 / 资质** | 杭州,航天信息系股份 | 北京,SaaS 报销 + 发票 | 深圳,航天信息子公司 |
| **税控对接** | 国家税务总局直连 | 通过百望 / 航信中转 | 国家税务总局直连 |
| **覆盖税种** | 增值税普票 / 专票 / 数电票 | 增值税普票 / 专票 | 增值税普票 / 专票 / 数电票 |
| **API 完备度** | ★★★★★ REST + SDK | ★★★ 偏 SaaS UI | ★★★★ REST |
| **跨境 / 海外公司** | 支持(签约境外主体) | 不支持(限内地法人) | 支持(港澳台 + 海外) |
| **价格(月费 / 单价)** | ¥99/月底费 + ¥0.5/张 | 按订阅,¥299/月起 | ¥199/月底费 + ¥0.3/张 |
| **开通耗时** | 3-7 工作日(走签合同) | 1 周(SaaS 配置) | 5-10 工作日 |
| **撤销 / 红字发票** | API 支持 | UI 操作 | API 支持 |
| **PDF + OFD 双格式** | ✓ | 仅 PDF | ✓ |

**推荐**: **诺诺网** — API 最完备 + 支持境外主体(若 attune 主体后续注册海外公司),单价最低。

## 3. 增值税普票 vs 专票 vs 数电票

| 类型 | 适用客户 | 抵扣 | 流转 |
|------|---------|------|------|
| **增值税普通发票**(纸 / 电子) | 个人 + 企业 | 不可抵扣进项税 | 邮件 PDF / OFD |
| **增值税专用发票**(电子专票) | 企业(一般纳税人) | 可抵扣 13% / 9% / 6% 进项 | 邮件 PDF + OFD + 章法 |
| **数电票(全电发票)** | 试点地区企业 + 个人 | 区分用途自动判断 | 国税总局电子发票服务平台直接送达 |

attune 默认开**增值税电子普通发票**(覆盖率最广,无须开票方有税控盘)。企业要求专票时由
人工后台切换 API endpoint 调专票接口。

### 3.1 个人 vs 企业 user 差异

| user 类型 | 必填字段 |
|----------|---------|
| **个人** | 抬头(姓名)+ 邮箱 |
| **企业(普票)** | 抬头(单位名)+ 纳税人识别号(统一社会信用代码)+ 邮箱 |
| **企业(专票)** | + 注册地址 + 电话 + 开户行 + 银行账号 |

UI 上 wizard 流程:
1. user 在 cloud accounts /me/billing 页面点「申请发票」
2. 选「个人 / 企业普票 / 企业专票」(radio)
3. 填字段(JS 实时校验税号格式:`/^[A-Z0-9]{18}$/`)
4. 提交 → 后台调诺诺 API 开票
5. 异步回调(诺诺 webhook)→ 写 InvoiceRecord + 发邮件给 user

## 4. 跨境特殊处理

attune 当前主体若在**海外**(Singapore / Delaware / HK 等):
- **不能**直接对中国客户开境内增值税发票(主体非内地法人)
- **方案 A**: 找内地代理公司(如义乌 / 香港 代理记账行)代理开票 — 加 6-10% 服务费
- **方案 B**: 国内设小规模纳税人主体,Stripe 收款后通过外贸合规渠道结汇 + 内地主体开票
  (复杂,需会计师事务所设计架构)
- **方案 C**: 改走「国内主体收款」 — Wechat Pay / Alipay / 支付宝商户号,绕过 Stripe

**短期(v1.0.x ~ v1.0.8)推荐方案 A**:与代理记账行签合同,人工开票模式,
attune 后台保留客户开票申请记录,定期(每周一)导出 CSV 给代理记账行批量开。

**中期(v1.1+)**: 评估方案 C 走支付宝 + 国内主体,降低人工成本。

## 5. API 集成范围(v1.0.8 实施)

### 5.1 DB schema

```python
# accounts/models.py 新加
class InvoiceRequest(Base):
    __tablename__ = "invoice_requests"
    id = Column(Integer, primary_key=True)
    user_id = Column(Integer, ForeignKey("users.id"), nullable=False)
    billing_event_id = Column(Integer, ForeignKey("billing_events.id"))
    invoice_type = Column(String(20))  # "personal" | "company_normal" | "company_special"
    buyer_name = Column(String(200))   # 抬头
    buyer_tax_no = Column(String(20))  # 纳税人识别号(企业必填)
    buyer_address = Column(String(500))  # 专票必填
    buyer_phone = Column(String(50))
    buyer_bank_name = Column(String(200))
    buyer_bank_account = Column(String(50))
    buyer_email = Column(String(200), nullable=False)
    status = Column(String(20))  # "pending" | "issued" | "failed" | "void"
    nuonuo_invoice_code = Column(String(50))  # 诺诺返回的发票代码
    nuonuo_invoice_no = Column(String(50))    # 发票号码
    pdf_url = Column(String(500))             # 诺诺托管的 PDF URL
    ofd_url = Column(String(500))             # OFD 格式 URL
    created_at = Column(DateTime, default=datetime.utcnow)
    issued_at = Column(DateTime)
```

### 5.2 API endpoints

```
POST /api/v1/billing/invoice-request  — user 提交开票申请
GET  /api/v1/billing/invoice-request/{id} — user 查申请状态
POST /api/v1/billing/nuonuo-webhook    — 诺诺回调(开票成功 / 失败)
POST /api/v1/admin/billing/invoice/{id}/void — 红字发票(管理员)
```

### 5.3 secrets/cloud.enc.yaml 增量

```yaml
nuonuo:
  app_key: <从诺诺开发者后台拿>
  app_secret: <从诺诺开发者后台拿>
  taxpayer_no: <开票方纳税人识别号(attune 主体)>
  endpoint: https://sdk.nuonuocs.cn/open  # 生产
  # endpoint: https://sandbox.nuonuocs.cn/open  # 沙箱
```

Per CLAUDE.md § Secrets 严禁硬编码 — `app_secret` 绝不入代码,统一 sops+age 加密。

## 6. 实施工作量估算

| Phase | 工作 | 工时 |
|-------|------|------|
| Phase 1 | supplier 选型 + 签合同 + 拿测试 API key | 1-2 周(business) |
| Phase 2 | DB schema + accounts API 实现 + 沙箱联调 | 3-5 天 |
| Phase 3 | UI 申请表单 + i18n + 状态轮询 | 2-3 天 |
| Phase 4 | 沙箱 E2E + 红字流程 + 异常恢复 | 2 天 |
| Phase 5 | 生产切换 + 监控告警 | 1 天 |

**总计**: 工程侧 ~10 天(business 流程并行),v1.0.8 release window 可覆盖。

## 7. 临时方案(v1.0.x 间)

`SUPPORT.md` 已包含:
> 如需中国增值税发票,请联系 `sales@engi-stack.com`,附:
> - Stripe charge id(在 `https://attune.engi-stack.com/me/billing` 查)
> - 开票类型(普票 / 专票)
> - 抬头 + 税号(企业)/ 姓名(个人)
> - 邮箱
>
> 我们将在 3 个工作日内安排开票并邮件发送 PDF + OFD。

## 8. 风险登记

| 风险 | 缓解 |
|------|------|
| 诺诺 API 服务质量(故障率 / 延迟) | 添加 retry + circuit breaker + 告警;高优先级 fallback 走人工开票 |
| 跨境合规(Stripe 收款 → 内地开票) | 需会计师事务所审计架构(方案 A / B / C 选定后实施) |
| user 输错税号 → 发票作废 | UI 实时校验 18 位统一社会信用代码格式 + 提交前预览确认 |
| 数据泄露(税号 + 银行账号) | DB 加密存储(per attune-pro Argon2id 模式)+ access log(per DSAR 要求) |
| 跨境主体合规(若 attune 主体海外) | 走方案 A(代理开票)规避,v1.1+ 评估方案 C |

## 9. 决策点(待 user 拍板)

1. **supplier 选型**: 诺诺 / 易快报 / 百望(默认推荐诺诺)
2. **主体方案**: 海外主体 + 代理开票 / 国内主体直开 / Stripe 替代方案(支付宝)
3. **v1.0.8 是否纳入**: 是 → 走 Phase 1-5 / 否 → 推 v1.1
4. **专票必要性**: v1.0.8 默认普票,v1.1 评估专票需求(取决于 B2B 客户占比)

## 10. 参考

- 诺诺开放平台: <https://open.nuonuo.com/>
- 国家税务总局电子发票公共服务平台: <https://fpdk.tax.gov.cn/>
- 国家税务总局 2020年第1号公告(电子专票推广)
- PIPL §28(敏感个人信息处理规则)
- attune § Secrets 严禁硬编码: `~/.claude/CLAUDE.md`
- attune-pro DSAR 实施: `attune-pro/accounts/api/dsar.py`(参考加密存储模式)
