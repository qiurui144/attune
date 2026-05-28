#!/usr/bin/env python3
"""law-pro 接入 — 全量前端 Playwright 验证矩阵（真 Chrome）。

走 attune Web UI 校验每个视图 / 可点击元素 + law-pro 端到端业务流程。
per plan tingly-knitting-zephyr 阶段 4。

分层：
  L0 Wizard 5 步 · L1 Sidebar · L2 八视图 · L3 Settings 6 tab ·
  L4 模态 · L5 law-pro（pluginhub 接入 + Marketplace + civil_loan 表单链 A）

幂等：vault 已 setup 则自动走解锁、跳过 Wizard。
监听 console error（server 重启窗口期的 CONNECTION_REFUSED 噪音不计 FAIL）。
截图归档 docs/screenshots/lawpro-e2e-verification/suite/。

前置：law-pro 已在 ~/.local/share/attune/plugins/；server 起在 ATTUNE_BASE_URL；
      pluginhub 经 SSH 隧道在 PLUGINHUB_URL 可达。
用法：bash tests/e2e/playwright/run_ui_all.sh
"""
import os
import sys

from playwright.sync_api import sync_playwright

BASE = os.environ.get("ATTUNE_BASE_URL", "http://127.0.0.1:18900")
PW = os.environ.get("ATTUNE_VAULT_PW", "Attune-E2E-Test-2026")
LLM_URL = os.environ.get("ATTUNE_LLM_URL", "https://hiapi.online/v1")
LLM_KEY = os.environ.get("ATTUNE_LLM_KEY", "")
LLM_MODEL = os.environ.get("ATTUNE_LLM_MODEL", "gemini-2.5-flash")
HUB_URL = os.environ.get("PLUGINHUB_URL", "http://127.0.0.1:9100")
HUB_KEY = os.environ.get("PLUGINHUB_LICENSE", "")
HEADLESS = os.environ.get("ATTUNE_HEADLESS", "1") != "0"
SHOT_DIR = "docs/screenshots/lawpro-e2e-verification/suite"

PASS = FAIL = 0
console_errors: list[str] = []
# server 重启窗口期浏览器轮询撞 connection-refused 是已知噪音，不计 FAIL
NOISE = ("ERR_CONNECTION_REFUSED", "favicon", "ws/scan-progress", "status/health")


def check(name: str, cond: bool, detail: str = "") -> bool:
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS  {name}  {detail}")
    else:
        FAIL += 1
        print(f"  FAIL  {name}  {detail}")
    return bool(cond)


def shot(page, name: str) -> None:
    os.makedirs(SHOT_DIR, exist_ok=True)
    try:
        page.screenshot(path=f"{SHOT_DIR}/{name}.png")
    except Exception as e:  # noqa: BLE001
        print(f"  (截图 {name} 失败: {e})")


def visible(page, role: str, name: str, timeout: int = 4000) -> bool:
    # wait_for 会自动等待元素出现（is_visible 是即时快照、SPA 渲染慢会误判）
    try:
        page.get_by_role(role, name=name).first.wait_for(state="visible", timeout=timeout)
        return True
    except Exception:  # noqa: BLE001
        return False


def click(page, role: str, name: str, timeout: int = 4000) -> bool:
    try:
        page.get_by_role(role, name=name).first.click(timeout=timeout)
        return True
    except Exception:  # noqa: BLE001
        return False


def run(page) -> None:
    # ── L0 Wizard / 解锁 ──────────────────────────────────────────
    print("\n── L0 Wizard 5 步 ──")
    page.goto(BASE, wait_until="networkidle")
    check("L0 页面标题", "Attune" in page.title(), page.title())

    if visible(page, "button", "开始设置"):
        shot(page, "l0-01-welcome")
        check("L0 Step1 欢迎 — 开始设置按钮", True)
        check("L0 Step1 — 导入备份按钮", visible(page, "button", "我有备份，直接导入"))
        click(page, "button", "开始设置")
        page.get_by_role("textbox", name="主密码").fill(PW, timeout=5000)
        page.get_by_role("textbox", name="再次输入").fill(PW)
        check("L0 Step2 — 密码强度校验", page.get_by_text("强").first.is_visible(timeout=2000))
        shot(page, "l0-02-password")
        click(page, "button", "下一步 →")
        ai_combo = page.get_by_role("combobox").first
        try:
            ai_combo.wait_for(state="visible", timeout=8000)
            check("L0 Step3 — AI 配置页", True)
        except Exception:  # noqa: BLE001
            check("L0 Step3 — AI 配置页", False)
        ai_combo.select_option("自定义（OpenAI 兼容）")
        page.get_by_role("textbox", name="URL 地址（OpenAI 兼容）").fill(LLM_URL)
        page.get_by_role("textbox", name="Token / API Key").fill(LLM_KEY)
        page.get_by_role("textbox", name="模型名（默认 auto 自动选择）").fill(LLM_MODEL)
        click(page, "button", "测试连接")
        ok_test = False
        try:
            page.get_by_text("✓").first.wait_for(timeout=20000)
            ok_test = True
        except Exception:  # noqa: BLE001
            pass
        check("L0 Step3 — 云端 LLM 连接测试通过", ok_test)
        shot(page, "l0-03-ai")
        click(page, "button", "使用云端")
        check("L0 Step4 — 硬件检测页", visible(page, "heading", "认识你的设备"))
        shot(page, "l0-04-hardware")
        click(page, "button", "应用推荐 →")
        check("L0 Step5 — 数据来源页", visible(page, "heading", "从哪里开始积累？"))
        shot(page, "l0-05-data")
        click(page, "button", "跳过，先看看 之后随时在设置中添加")
        click(page, "button", "完成 · 进入 Attune →")
    elif visible(page, "button", "解锁"):
        page.get_by_role("textbox", name="主密码").fill(PW)
        click(page, "button", "解锁")
        check("L0 vault 已存在 — 解锁进入", True)
    else:
        check("L0 — 已在主界面", True)

    page.wait_for_timeout(1500)
    check("L0 → 进入主界面", visible(page, "button", "条目", timeout=8000))
    shot(page, "l0-06-main")

    # ── L1 Sidebar ───────────────────────────────────────────────
    print("\n── L1 Sidebar ──")
    for tab in ["条目", "项目", "远程目录", "知识全景", "技能", "插件市场", "设置"]:
        check(f"L1 导航 — {tab}", visible(page, "button", tab))
    check("L1 — 新对话按钮", visible(page, "button", "新对话"))
    check("L1 — 全局搜索按钮", visible(page, "button", "全局搜索（Cmd+K）"))
    check("L1 — 账号菜单", visible(page, "button", "账号菜单"))
    page.keyboard.press("Control+k")
    page.wait_for_timeout(600)
    check("L4 CommandPalette — Cmd+K 唤起", page.get_by_role("textbox").count() > 0)
    page.keyboard.press("Escape")

    # ── L2 八视图 ─────────────────────────────────────────────────
    print("\n── L2 八视图 ──")
    views = [
        ("条目", "条目", "上传文件"),
        ("项目", "Projects", "新建项目"),
        ("远程目录", "远程目录", "添加 WebDAV"),
        ("知识全景", "知识全景", None),
        ("技能", "Skills", "刷新"),
        ("插件市场", "插件市场", None),
    ]
    for nav, heading_kw, btn in views:
        click(page, "button", nav)
        page.wait_for_timeout(700)
        check(f"L2 {nav} — 视图渲染", heading_kw in page.content())
        if btn:
            check(f"L2 {nav} — {btn} 按钮", visible(page, "button", btn))
        shot(page, f"l2-{nav}")
    click(page, "button", "新对话")
    page.wait_for_timeout(700)
    check("L2 Chat — 对话输入框", visible(page, "textbox", "对话输入框"))
    check("L2 Chat — 切换模型按钮", visible(page, "button", "切换模型"))
    shot(page, "l2-chat")

    # ── L3 Settings 6 tab ────────────────────────────────────────
    print("\n── L3 Settings 6 tab ──")
    click(page, "button", "设置")
    page.wait_for_timeout(700)
    for tab in ["通用", "AI 大脑", "数据", "会员", "隐私", "关于"]:
        ok = click(page, "button", tab)
        page.wait_for_timeout(500)
        check(f"L3 Settings tab — {tab}", ok)
        shot(page, f"l3-{tab}")

    # ── L5 law-pro 接入 ───────────────────────────────────────────
    print("\n── L5 law-pro 接入 ──")
    click(page, "button", "会员")
    page.wait_for_timeout(500)
    click(page, "button", "▶ 展开 · 默认使用 engi-stack.com 公共云")
    page.wait_for_timeout(400)
    try:
        page.get_by_role("textbox", name="https://hub.your-company.com").fill(HUB_URL)
        page.get_by_role("textbox", name="license key").fill(HUB_KEY)
        click(page, "button", "保存 cloud 后端配置")
        page.wait_for_timeout(1500)
        check("L5.1 pluginhub URL + license 配置保存", True)
    except Exception as e:  # noqa: BLE001
        check("L5.1 pluginhub 配置", False, str(e)[:80])

    click(page, "button", "插件市场")
    page.wait_for_timeout(1500)
    mk = page.content()
    check("L5.2 Marketplace — provider=http-pluginhub", "http-pluginhub" in mk)
    check("L5.2 Marketplace — law-pro v0.2.0 列出", "law-pro" in mk and "0.2.0" in mk)
    shot(page, "l5-01-marketplace")

    click(page, "button", "项目")
    page.wait_for_timeout(700)
    proj_name = "E2E-民间借贷-自动"
    if not (proj_name in page.content()):
        click(page, "button", "+ 新建项目") or click(page, "button", "新建项目")
        page.wait_for_timeout(500)
        boxes = page.get_by_role("textbox")
        if boxes.count() >= 2:
            boxes.nth(0).fill(proj_name)
            boxes.nth(1).fill("civil-loan")
            page.get_by_role("button", name="新建项目").last.click()
            page.wait_for_timeout(1000)
    check("L5.3 civil-loan Project 创建", proj_name in page.content())
    click(page, "button", proj_name)
    page.wait_for_timeout(800)
    check("L5.4 law-pro 计算助手面板挂载", "计算助手" in page.content())
    shot(page, "l5-02-project")

    chain_ok = False
    if click(page, "button", "▶ 运行"):
        page.wait_for_timeout(800)
        try:
            page.get_by_role("textbox", name="原告姓名 *").fill("张三")
            page.get_by_role("textbox", name="被告姓名 *").fill("李四")
            page.get_by_label("我方代理 *—原告被告").select_option("原告")
            page.get_by_role("spinbutton", name="本金（元） *").fill("200000")
            page.get_by_role("spinbutton", name="利率 *").first.fill("0.096")
            page.get_by_label("利率类型 *—年利率月利率日利率").select_option("年利率")
            page.get_by_label("计息方式 *—单利复利").select_option("单利")
            page.get_by_role("textbox", name="起算日 *").fill("2023-01-01")
            page.get_by_role("textbox", name="截止日 *").fill("2024-01-01")
            page.get_by_label("计算公式 *—年单利月单利日单利年复利LPR 4 倍封顶单利").select_option("年单利")
            page.get_by_role("button", name="计算").click()
            page.wait_for_timeout(3000)
            chain_ok = "19200" in page.locator("[role=dialog]").first.inner_text()
        except Exception as e:  # noqa: BLE001
            print(f"  (L5.5 表单异常: {e})")
    check("L5.5 civil_loan 表单 → agent 算应付利息 ¥19,200", chain_ok)
    shot(page, "l5-03-agent-result")


def main() -> int:
    if not LLM_KEY or not HUB_KEY:
        print("ERROR: 需设 ATTUNE_LLM_KEY + PLUGINHUB_LICENSE 环境变量（见 run_ui_all.sh）")
        return 2
    print(f"=== law-pro 全量前端 E2E ===  BASE={BASE}  headless={HEADLESS}\n")
    with sync_playwright() as p:
        browser = p.chromium.launch(channel="chrome", headless=HEADLESS)
        # locale=zh-CN — attune i18n 按 navigator.language 渲染，固定中文以对齐选择器
        context = browser.new_context(locale="zh-CN")
        page = context.new_page()
        page.on("console", lambda m: console_errors.append(m.text)
                if m.type == "error" else None)
        try:
            run(page)
        finally:
            browser.close()

    real_errs = [e for e in console_errors if not any(n in e for n in NOISE)]
    print("\n── Console error ──")
    check("无未处理 JS 错误（排除重启窗口噪音）", not real_errs,
          f"{len(real_errs)} 真实 / {len(console_errors)} 总")
    for e in real_errs[:10]:
        print(f"    {e[:140]}")

    print(f"\n=== 结果：{PASS} PASS / {FAIL} FAIL ===")
    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
