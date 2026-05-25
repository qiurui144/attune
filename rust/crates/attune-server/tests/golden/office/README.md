# Office Helper Golden Dataset

> Spec: `docs/superpowers/specs/2026-05-20-office-helper-design.md` §6
> Plan: `docs/superpowers/plans/2026-05-20-office-helper.md` §D3

## 目录结构

```
golden/office/
├── BASELINE_ENV.md          # 测试环境基线 + 红线表
├── README.md                # 本文件 — 样本收集流程
├── benchmarks/              # accuracy/speed 历史快照 (CI 自动写入)
├── ocr/
│   ├── document/<id>.png + <id>.expected.yaml    # 10 样本
│   ├── receipt/             # 10 样本
│   ├── table/               # 10 样本
│   ├── card/                # 10 样本 (Z 高标杆 92%)
│   ├── id_card_cn/          # 5 样本
│   ├── bank_card/           # 5 样本
│   └── business_license/    # 5 样本
└── asr/
    ├── zh_aishell/<id>.wav + <id>.expected.yaml  # 20 段
    ├── en_libri/            # 20 段
    ├── zh_en_mixed/         # 5 段
    └── meeting/             # 10 段 (DER 测试用)
```

**总计**: 50 OCR sample + 55 ASR sample.

## 样本收集策略 (per CLAUDE.md benchmark 铁律)

### OCR 样本来源 — 公开数据 + 内部脱敏

| Scene | 来源建议 | 公开数据集 |
|-------|---------|-----------|
| document | 内部 PDF (脱敏) / arXiv 双栏论文 / 政府公开报告 | 无标杆公开集 |
| receipt | 内部增值税普票/专票/电子票 (脱敏) | SROIE 2019 (英文) |
| table | 财务报表打印件 / Excel 截图 (脱敏) | FUNSD (英文表单) |
| card | 商务名片 / 设计师名片 (脱敏) | 无标杆公开集 |
| id_card_cn | **合成数据**（用 GB 11643 校验位生成器，无真实证件）| — |
| bank_card | **合成数据**（用 Luhn 生成器，无真实卡号） | — |
| business_license | **合成数据**（用 GB 32100 校验位生成器） | — |

**红线**:
- ❌ **禁止** 上传真实身份证 / 银行卡 / 营业执照原图 (隐私 + 法律风险)
- ❌ **禁止** np.random 合成 OCR 图像 (per CLAUDE.md "数据集铁律")
- ✅ 卡证类用 GB/Luhn 合规生成器 + Photoshop/ImageMagick 渲染到模板（不是真实图像）
- ✅ 商业发票/名片可上传，但**必须脱敏**（人名/电话/地址替换为虚构值）

### ASR 样本来源 — 公开集为主

| Lang/Scene | 来源 |
|-----------|------|
| zh_aishell | AISHELL-3 (CC BY-NC-SA 4.0, [magicdata.com](http://www.magicdata.com)) 抽 20 段 |
| en_libri | LibriSpeech test-clean (CC BY 4.0, [openslr.org](http://www.openslr.org/12)) 抽 20 段 |
| zh_en_mixed | 内部中英混说技术分享 (脱敏：脱去公司/产品名) |
| meeting | 内部会议录音 (2-4 人, 5-15 分钟/段，脱敏后) |

**版权声明**: 仓内不直接放 AISHELL/LibriSpeech 原始 audio (各自 license)，
而是放 wav 文件的**指纹哈希 (SHA256) + 期望文本** 在 `<id>.expected.yaml`，
配套下载脚本 `scripts/fetch-office-asr-golden.sh` 从公开镜像下载（CI 不跑下载，
本地开发跑测试前先 fetch）。

## expected.yaml 格式 (OCR)

```yaml
id: doc-receipt-01
profile: receipt
schema_version: receipt_v1
expected_fields:
  invoice_no: "12345678"
  issue_date: "2026-05-18"
  seller: "ABC科技有限公司"
  buyer: "XYZ咨询有限公司"
  amount_total: "1234.56"
  tax_amount: "111.11"
  amount_chinese: "壹仟贰佰叁拾肆元伍角陆分"
expected_lines_count_min: 12  # OCR 引擎至少识别这么多行
max_elapsed_ms: 2000          # 速度上限红线 (per BASELINE_ENV)
reviewer:
  name: REAL_INVOICE_ANONYMIZED  # or SYNTHETIC_GB_VALID for cards
  approved: true
notes: |
  增值税电子普通发票 (2026-05-18) — 脱敏: 公司名替换 / 金额保留.
```

## expected.yaml 格式 (ASR)

```yaml
id: aishell-zh-01
audio_sha256: "abc123def456..."  # 文件指纹 (避免直接放 wav)
audio_url: "https://www.openslr.org/resources/93/aishell3.tar.gz"  # 公开镜像
duration_sec: 8.23
language: zh
expected_transcript: "今天 天气 真好 我们 去 公园 散步"
expected_speakers: 1            # 单人 → DER 不测
reviewer:
  name: AISHELL3_PUBLIC
  approved: true
```

## D3 实施现状 (2026-05-21)

D3.1 完成: 目录 + BASELINE_ENV.md + 本 README + 数据集采集脚本骨架。

**实际样本**: 当前**仓内零图片 / 零音频**，只有目录 + 元信息。
样本由 D3.2/D3.3 期间逐步补：
- 先用 synthetic 样本（卡证 GB-valid 生成 + 简单 SVG→PNG 渲染）跑通 gate 框架
- 再补内部脱敏样本 (用户/审计补)
- ASR 走 sha256 + 下载脚本路径，不直接 commit wav

## 跟 spec/plan 的对应

- `BASELINE_ENV.md` 红线表对应 spec §5
- 目录结构对应 plan §D3 + spec §6.2
- 样本来源策略对应 plan §D3.1 (\"公开集自动化 + 内部脱敏\")
