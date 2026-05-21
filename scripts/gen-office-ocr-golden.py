#!/usr/bin/env python3
"""
Generate OCR synthetic golden samples for office-helper L1 gate.

每个 scene 生成若干合规的 sample, 不依赖外网下载, 不上传真实证件/卡号.
卡证类使用 GB 11643 / Luhn / GB 32100 合法校验位生成器.

Usage:
  python3 scripts/gen-office-ocr-golden.py [--scenes receipt,id_card_cn,bank_card,business_license]

输出:
  rust/crates/attune-server/tests/golden/office/ocr/<scene>/syn-<N>.png + syn-<N>.expected.yaml

为简单起见,本脚本只生成 expected.yaml + 简单 ASCII / PIL 渲染 (cards/receipts 文本布局).
真正高保真样本由后续内部脱敏样本补充.
"""
from __future__ import annotations
import argparse
import json
import os
import random
import string
import sys
from datetime import date
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
OCR_DIR = REPO_ROOT / "rust/crates/attune-server/tests/golden/office/ocr"

random.seed(42)  # 固定种子, 保证 CI reproducibility


# ─── GB / Luhn 校验位生成器 ─────────────────────────────────────────

def gb11643_check_digit(id17: str) -> str:
    """GB 11643-1999 居民身份证号 18 位校验位."""
    assert len(id17) == 17 and id17.isdigit()
    weights = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2]
    check_chars = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2']
    s = sum(int(d) * w for d, w in zip(id17, weights))
    return check_chars[s % 11]


def luhn_check_digit(card15_to_18: str) -> str:
    """Luhn 校验位 (输入 N-1 位, 返回最后一位)."""
    digits = [int(d) for d in card15_to_18]
    # 反向, 偶数位 (从右起 0-indexed) double
    s = 0
    for i, d in enumerate(reversed(digits)):
        if i % 2 == 0:  # 待填位置一定是 even-from-right 的第一个 (i=0 reserved)
            doubled = d * 2
            s += doubled - 9 if doubled > 9 else doubled
        else:
            s += d
    # 待填位置使总和 mod 10 == 0
    return str((10 - s % 10) % 10)


def gb32100_check_digit(code17: str) -> str:
    """GB 32100-2015 统一社会信用代码校验位."""
    alphabet = "0123456789ABCDEFGHJKLMNPQRTUWXY"
    weights = [1, 3, 9, 27, 19, 26, 16, 17, 20, 29, 25, 13, 8, 24, 10, 30, 28]
    total = 0
    for ch, w in zip(code17, weights):
        pos = alphabet.find(ch.upper())
        if pos < 0:
            raise ValueError(f"invalid char {ch} not in alphabet {alphabet}")
        total += pos * w
    expected_pos = (31 - total % 31) % 31
    return alphabet[expected_pos]


# ─── Scene generators (output expected.yaml only; PNG generation is deferred) ───

def gen_receipt(n: int) -> dict:
    """生成发票合成数据 (expected.yaml only — 图片在 D3 后期手工渲染)."""
    invoice_no = ''.join(random.choices(string.digits, k=8))
    issue_date = date(2026, random.randint(1, 5), random.randint(1, 28)).isoformat()
    amount = round(random.uniform(100, 10000), 2)
    tax_rate = 0.13
    tax = round(amount * tax_rate / (1 + tax_rate), 2)
    chinese_amount = _to_chinese_amount(amount)
    return {
        "id": f"syn-receipt-{n}",
        "profile": "receipt",
        "schema_version": "receipt_v1",
        "expected_fields": {
            "invoice_no": invoice_no,
            "issue_date": issue_date,
            "seller": f"测试销售方 {n} 有限公司",
            "buyer": f"测试购买方 {n} 有限公司",
            "amount_total": f"{amount:.2f}",
            "tax_amount": f"{tax:.2f}",
            "amount_chinese": chinese_amount,
        },
        "expected_lines_count_min": 8,
        "max_elapsed_ms": 2000,
        "reviewer": {"name": "SYNTHETIC_GENERATED", "approved": True},
        "notes": "Synthetic invoice — fake company names, mathematically consistent amounts.",
    }


def gen_id_card_cn(n: int) -> dict:
    """生成中国身份证合成数据 (GB 11643 valid)."""
    # 17 位前缀: 地区(6) + 出生(8) + 顺序(3)
    region = random.choice(["110101", "310101", "440101", "510101", "330101"])
    yy = random.randint(1970, 2000)
    mm = random.randint(1, 12)
    dd = random.randint(1, 28)
    birth = f"{yy:04d}{mm:02d}{dd:02d}"
    seq = ''.join(random.choices(string.digits, k=3))
    id17 = region + birth + seq
    id_full = id17 + gb11643_check_digit(id17)
    return {
        "id": f"syn-id_card_cn-{n}",
        "profile": "id_card",
        "id_card_subtype": "id_card_cn",
        "schema_version": "id_card_cn_v1",
        "expected_fields": {
            "name": f"合成姓名 {n}",
            "gender": random.choice(["男", "女"]),
            "nationality": "汉",
            "birth_date": f"{yy:04d}-{mm:02d}-{dd:02d}",
            "address": f"测试省测试市测试区测试街道 {n} 号",
            "id_number": id_full,
        },
        "expected_lines_count_min": 6,
        "max_elapsed_ms": 2000,
        "reviewer": {"name": "SYNTHETIC_GB11643_VALID", "approved": True},
        "notes": "Synthetic — GB 11643 check digit verified; no real identity.",
    }


def gen_bank_card(n: int) -> dict:
    """生成银行卡合成数据 (Luhn valid)."""
    # 16 位 BIN + 自由位
    bin6 = random.choice(["622578", "622588", "455614", "424519", "356896"])
    middle = ''.join(random.choices(string.digits, k=9))
    card15 = bin6 + middle
    card16 = card15 + luhn_check_digit(card15)
    valid_thru = f"{random.randint(1, 12):02d}/{random.randint(26, 30)}"
    return {
        "id": f"syn-bank_card-{n}",
        "profile": "id_card",
        "id_card_subtype": "bank_card",
        "schema_version": "bank_card_v1",
        "expected_fields": {
            "card_number": ' '.join(card16[i:i+4] for i in range(0, 16, 4)),
            "bank_name": random.choice(["中国工商银行", "中国建设银行", "招商银行", "中国银行"]),
            "card_type": random.choice(["借记卡", "信用卡"]),
            "valid_thru": valid_thru,
        },
        "expected_lines_count_min": 4,
        "max_elapsed_ms": 2000,
        "reviewer": {"name": "SYNTHETIC_LUHN_VALID", "approved": True},
        "notes": "Synthetic — Luhn check digit verified; no real card.",
    }


def gen_business_license(n: int) -> dict:
    """生成营业执照合成数据 (GB 32100 valid)."""
    alphabet = "0123456789ABCDEFGHJKLMNPQRTUWXY"
    # 17 位: 登记管理部门(1) + 机构类别(1) + 区划(6) + 组织机构代码(9)
    code17 = ''.join(random.choices(alphabet, k=17))
    code_full = code17 + gb32100_check_digit(code17)
    yy = random.randint(2010, 2025)
    mm = random.randint(1, 12)
    dd = random.randint(1, 28)
    return {
        "id": f"syn-business_license-{n}",
        "profile": "id_card",
        "id_card_subtype": "business_license",
        "schema_version": "business_license_v1",
        "expected_fields": {
            "registration_no": code_full,
            "company_name": f"合成测试 {n} 有限公司",
            "legal_rep": f"测试法人 {n}",
            "registered_capital": f"{random.randint(10, 1000)} 万元人民币",
            "established_date": f"{yy:04d}-{mm:02d}-{dd:02d}",
            "scope": "技术开发、技术咨询、软件销售",
        },
        "expected_lines_count_min": 8,
        "max_elapsed_ms": 2000,
        "reviewer": {"name": "SYNTHETIC_GB32100_VALID", "approved": True},
        "notes": "Synthetic — GB 32100 check digit verified; no real entity.",
    }


def _to_chinese_amount(amt: float) -> str:
    """简化版 number → 大写金额; 仅支持 < 100,000.00 (足够 demo)."""
    yuan = int(amt)
    cents = round((amt - yuan) * 100)
    jiao = cents // 10
    fen = cents % 10
    digits = "零壹贰叁肆伍陆柒捌玖"
    units = ["", "拾", "佰", "仟", "万", "拾", "佰", "仟"]
    s = ""
    if yuan == 0:
        s = "零元"
    else:
        ystr = str(yuan)
        n = len(ystr)
        for i, ch in enumerate(ystr):
            pos = n - 1 - i
            d = int(ch)
            if d == 0:
                if not s.endswith("零") and pos > 0:
                    s += "零"
            else:
                s += digits[d] + units[pos]
        s += "元"
    if jiao:
        s += digits[jiao] + "角"
    if fen:
        s += digits[fen] + "分"
    if not jiao and not fen:
        s += "整"
    return s


# ─── Driver ─────────────────────────────────────────────────────

GENERATORS = {
    "receipt": (gen_receipt, 5),       # 5 个 synthetic + 5 个内部脱敏 (TODO)
    "id_card_cn": (gen_id_card_cn, 5),
    "bank_card": (gen_bank_card, 5),
    "business_license": (gen_business_license, 5),
}

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--scenes", default=",".join(GENERATORS.keys()),
                        help="comma-separated scenes to generate")
    parser.add_argument("--force", action="store_true",
                        help="overwrite existing files")
    args = parser.parse_args()

    requested = set(args.scenes.split(","))
    unknown = requested - set(GENERATORS.keys())
    if unknown:
        print(f"unknown scenes: {unknown}; valid = {list(GENERATORS.keys())}", file=sys.stderr)
        sys.exit(2)

    import yaml  # PyYAML; pip install pyyaml

    total = 0
    for scene in sorted(requested):
        gen_fn, count = GENERATORS[scene]
        scene_dir = OCR_DIR / scene
        scene_dir.mkdir(parents=True, exist_ok=True)
        for i in range(1, count + 1):
            data = gen_fn(i)
            yaml_path = scene_dir / f"{data['id']}.expected.yaml"
            if yaml_path.exists() and not args.force:
                print(f"[skip] {yaml_path.relative_to(REPO_ROOT)} (exists)")
                continue
            yaml_path.write_text(
                yaml.safe_dump(data, allow_unicode=True, sort_keys=False),
                encoding="utf-8",
            )
            total += 1
            print(f"[gen]  {yaml_path.relative_to(REPO_ROOT)}")
    print(f"\nGenerated {total} new expected.yaml files.")
    print("Note: PNG rendering deferred — D3 gate tests will skip samples without images.")


if __name__ == "__main__":
    main()
