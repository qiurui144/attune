#!/usr/bin/env python3
"""law-pro 接入 — 复杂证据链 + 金额计算 E2E。

通过 attune 的 POST /api/v1/agents/civil_loan_agent/run 路由（agent-run），
验证 civil_loan_agent 在 3 条复杂证据链下金额计算正确 + 证据链完整性评估。

3 条链（复杂度递增）：
- 链A 标准      : lawcontrol 借款合同样本 20万/年9.6%/1年 → 单利
- 链B 砍头息    : 借条50万/转账实付45万 → 本金按实付计（民法典680条）
- 链C 利率红线  : 100万/年36%超LPR4倍 → lpr_capped 封顶 + 部分还款冲抵

前置：law-pro 已 plugin-install 接入；server 起在 :18930（XDG 隔离）。
用法：python3 tests/e2e/lawpro_chains_e2e.py  → 期望 12 PASS / 0 FAIL"""
import json
import os
import sys
import urllib.error
import urllib.request

BASE = os.environ.get("ATTUNE_BASE_URL", "http://localhost:18930")
PW = "lawpro-e2e-2026"
PASS = 0
FAIL = 0


def post(path, body):
    r = urllib.request.Request(BASE + path, data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            return resp.status, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read().decode())
        except Exception:
            return e.code, {}


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS  {name}  {detail}")
    else:
        FAIL += 1
        print(f"  FAIL  {name}  {detail}")


def facts(**kw):
    base = {
        "parties": {"plaintiff_name": "梁素燕", "defendant_name": "任其坤",
                    "our_client_is": "plaintiff", "case_no": "(2025)京01民初8888号"},
        "principal": None, "principal_evidence": "",
        "rate_value": None, "rate_evidence": "", "rate_type": "year", "rate_method": "simple",
        "start_date": None, "start_date_evidence": "", "end_date": None, "end_date_evidence": "",
        "paid_interest": 0.0, "paid_interest_evidence": "无",
        "paid_principal": 0.0, "paid_principal_evidence": "无",
        "loan_doc_exists": True, "loan_doc_evidence": "借条已存档",
        "recommended_formula": "simple_interest_year", "formula_reason": "年息单利", "warnings": [],
    }
    base.update(kw)
    return base


def run_agent(f, evidence=None):
    return post("/api/v1/agents/civil_loan_agent/run",
                {"input": {"facts": f, "classified_evidence": evidence or []}})


print("=== law-pro 复杂证据链 + 金额计算 E2E ===\n")
post("/api/v1/vault/setup", {"password": PW})
post("/api/v1/vault/unlock", {"password": PW})

# 链 A — 标准基线（lawcontrol 借款合同样本）
print("链 A — 标准 (合同样本 20万/年9.6%/1年)")
st, d = run_agent(facts(
    principal=200000.0, principal_evidence="借款合同_民间借贷.txt 第1条 本金20万",
    rate_value=0.096, rate_evidence="合同第2条 年化9.6%",
    start_date="2024-01-01", start_date_evidence="合同签订日",
    end_date="2025-01-01", end_date_evidence="约定还款日"))
cA = d.get("output", {}).get("computation", {})
check("链A agent-run HTTP 200", st == 200, f"st={st}")
check("链A 公式=simple_interest_year", cA.get("formula_used") == "simple_interest_year")
check("链A 应付利息=¥19252.6", cA.get("computed_interest") == 19252.6, str(cA.get("computed_interest")))
check("链A 应收余额=¥219252.6", cA.get("remaining_balance") == 219252.6, str(cA.get("remaining_balance")))

# 链 B — 砍头息（本金按实付计）
print("\n链 B — 砍头息 (借条50万/实付45万/年24%)")
st, d = run_agent(facts(
    principal=450000.0,
    principal_evidence="借条载明50万，转账流水实付45万，差额5万系砍头息，依民法典680条本金按实付45万计",
    rate_value=0.24, rate_evidence="借条约定年利率24%",
    start_date="2023-06-01", start_date_evidence="转账凭证放款日",
    end_date="2025-05-01", end_date_evidence="起诉日"))
cB = d.get("output", {}).get("computation", {})
check("链B 本金按实付45万计利息=¥207123.29", cB.get("computed_interest") == 207123.29, str(cB.get("computed_interest")))
check("链B 应收余额=¥657123.29", cB.get("remaining_balance") == 657123.29, str(cB.get("remaining_balance")))
check("链B audit_trail 含本金依据", "45万" in d.get("output", {}).get("audit_trail", ""))

# 链 C — 利率超 LPR 红线 + 部分还款冲抵
print("\n链 C — 利率红线+部分还款 (100万/年36%/已还本20万息5万)")
st, d = run_agent(facts(
    principal=1000000.0, principal_evidence="借条本金100万",
    rate_value=0.36, rate_evidence="借条约定年利率36%",
    start_date="2022-03-01", start_date_evidence="放款日",
    end_date="2025-05-01", end_date_evidence="起诉日",
    paid_interest=50000.0, paid_interest_evidence="2023年还息流水5万",
    paid_principal=200000.0, paid_principal_evidence="2024年还本流水20万",
    recommended_formula="lpr_capped_simple", formula_reason="约定36%超LPR4倍按司法保护上限封顶"))
cC = d.get("output", {}).get("computation", {})
check("链C 公式=lpr_capped_simple (利率封顶)", cC.get("formula_used") == "lpr_capped_simple")
# LPR 按起息日(2022-03-01)查表 → 1年期 LPR 3.70% × 4 = 14.8% 封顶
# （旧值基于写死的 2024 LPR×4=13.8%，已随 date-aware LPR 修正）
check("链C 应付利息=¥469139.73 (2022 LPR×4=14.8% 封顶)",
      cC.get("computed_interest") == 469139.73, str(cC.get("computed_interest")))
check("链C 应收余额扣已还=¥1219139.73",
      cC.get("remaining_balance") == 1219139.73, str(cC.get("remaining_balance")))

# 证据链完整性 — 传 classified_evidence 应消除 missing_evidence
print("\n证据链完整性 — classified_evidence 消除待补提示")
st, d_noev = run_agent(facts(
    principal=100000.0, principal_evidence="借条", rate_value=0.12, rate_evidence="借条",
    rate_type="year", start_date="2024-01-01", start_date_evidence="x",
    end_date="2025-01-01", end_date_evidence="x"))
miss_noev = len(d_noev.get("output", {}).get("missing_evidence", []))
st, d_ev = run_agent(facts(
    principal=100000.0, principal_evidence="借条", rate_value=0.12, rate_evidence="借条",
    rate_type="year", start_date="2024-01-01", start_date_evidence="x",
    end_date="2025-01-01", end_date_evidence="x"),
    evidence=[
        {"file": "evi_005_iou.png", "kind": "borrowing_doc", "confidence": 0.95},
        {"file": "工商银行流水清单.pdf", "kind": "bank_statement", "confidence": 0.9},
    ])
miss_ev = len(d_ev.get("output", {}).get("missing_evidence", []))
check("无证据时有 missing_evidence 提示", miss_noev > 0, f"missing={miss_noev}")
check("传证据链后 missing_evidence 减少", miss_ev < miss_noev, f"{miss_noev} → {miss_ev}")

print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
sys.exit(0 if FAIL == 0 else 1)
