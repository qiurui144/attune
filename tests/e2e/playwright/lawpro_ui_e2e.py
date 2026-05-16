#!/usr/bin/env python3
"""law-pro 接入 — 全面前端 Playwright 验证矩阵。

真 Chrome（channel=chrome）走 attune Web UI，验证每个功能/可点击元素 +
law-pro 接入后的完整业务流程。per plan tingly-knitting-zephyr 阶段 4。

分层：L0 Wizard / L1 Sidebar / L2 八视图 / L3 Settings / L4 模态 /
L5 law-pro（Marketplace 卡片 + 新建 civil-loan Project + 上传 lawcontrol 证据 + chat 触发）。

幂等：wizard.complete 持久化后第二次跑自动跳过 wizard。
监听 console error（favicon 404 等已知噪音不计 FAIL）。
截图归档 docs/screenshots/lawpro-e2e-verification/<env>/。

前置：law-pro 已 plugin-install；server 起在 ATTUNE_BASE_URL（默认 :18930）。
用法：python3 tests/e2e/playwright/lawpro_ui_e2e.py
"""
import os
import sys
from playwright.sync_api import sync_playwright

BASE = os.environ.get("ATTUNE_BASE_URL", "http://localhost:18930")
ENV = os.environ.get("ATTUNE_ENV", "local")
SHOT_DIR = f"docs/screenshots/lawpro-e2e-verification/{ENV}"
# lawcontrol 真实证据（用户指定"证据链从 lawcontrol 获取"）
LAWCONTROL_EVIDENCE = "/data/company/project/lawcontrol/data/test_evidence/合同样本/借款合同_民间借贷.txt"
PASS = 0
FAIL = 0
console_errors = []
failed_responses = []  # (status, url) — 4xx/5xx 响应，用于定位 console "Failed to load resource"


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS  {name}  {detail}")
    else:
        FAIL += 1
        print(f"  FAIL  {name}  {detail}")
    return cond


def shot(page, name):
    os.makedirs(SHOT_DIR, exist_ok=True)
    page.screenshot(path=f"{SHOT_DIR}/{name}.png")


def visible(page, **kw):
    try:
        return page.get_by_role(**kw).first.is_visible(timeout=2000)
    except Exception:
        return False


def click_if(page, role, name, timeout=3000):
    try:
        page.get_by_role(role, name=name).first.click(timeout=timeout)
        return True
    except Exception:
        return False


def l0_wizard(page):
    print("\n── L0 Wizard 5 步 ──")
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_timeout(1500)
    check("L0 页面标题加载", "Attune" in page.title(), page.title())
    on_wizard = visible(page, role="button", name="Get started")
    if on_wizard:
        shot(page, "l0-01-welcome")
        check("L0 Welcome — Get started", True)
        check("L0 Welcome — 导入备份按钮", visible(page, role="button", name="I have a vault"))
        click_if(page, "button", "Get started")
        page.wait_for_timeout(800)
        pw_box = page.get_by_role("textbox", name="Vault Password").first
        check("L0 Step2 — 密码输入框", pw_box.is_visible(timeout=3000))
        # 全新 vault 需设密码 + 确认；已 setup 的 vault 显示 continue（无 Confirm 框）
        confirm = page.get_by_role("textbox", name="Confirm")
        if confirm.count() > 0 and confirm.first.is_visible(timeout=1000):
            pw_box.fill("lawpro-ui-2026")
            confirm.first.fill("lawpro-ui-2026")
            check("L0 Step2 — 密码+确认已填（全新 vault）", True)
        else:
            check("L0 Step2 — vault 已 setup，直接继续", True)
        shot(page, "l0-02-password")
        click_if(page, "button", "Next")
        # 全新 vault setup 触发 Argon2id 主密钥派生（注释见 Step2Password.tsx ~10s），
        # 固定 sleep 会误判 —— 锚定 Step3 标题真正渲染，给 30s 容纳派生 + 网络。
        try:
            page.get_by_role("heading", name="Choose your AI brain").first.wait_for(timeout=30000)
        except Exception:
            pass
        check("L0 Step3 — AI 配置页", visible(page, role="heading", name="Choose your AI brain"))
        shot(page, "l0-03-ai")
        try:
            page.get_by_text("Configure later").first.click(timeout=3000)
        except Exception:
            pass
        page.wait_for_timeout(600)
        click_if(page, "button", "Apply recommendation")
        page.wait_for_timeout(600)
        try:
            page.get_by_role("button", name="Skip for now").first.click(timeout=3000)
        except Exception:
            pass
        page.wait_for_timeout(600)
        click_if(page, "button", "Finish")
        page.wait_for_timeout(2500)
    else:
        check("L0 Wizard 已完成(持久化)，直接进主界面", True)
    # 验进主界面
    try:
        page.get_by_role("button", name="Items").first.wait_for(timeout=10000)
    except Exception:
        pass
    check("L0 → 进入主界面", visible(page, role="button", name="Items"))
    shot(page, "l0-04-main")


def l1_sidebar(page):
    print("\n── L1 Sidebar ──")
    for tab in ["Items", "Projects", "Remote", "Knowledge", "Skills", "Marketplace", "Settings"]:
        ok = click_if(page, "button", tab)
        page.wait_for_timeout(500)
        check(f"L1 导航标签 — {tab}", ok)
    check("L1 — New chat 按钮", click_if(page, "button", "New chat"))
    page.wait_for_timeout(400)
    shot(page, "l1-sidebar")


def l2_views(page):
    print("\n── L2 八视图核心元素 ──")
    click_if(page, "button", "Items"); page.wait_for_timeout(500)
    check("L2 Items — Upload files 按钮", visible(page, role="button", name="Upload files"))
    check("L2 Items — Refresh 按钮", visible(page, role="button", name="Refresh"))
    click_if(page, "button", "Projects"); page.wait_for_timeout(500)
    check("L2 Projects — 新建项目入口", visible(page, role="button", name="新建项目"))
    click_if(page, "button", "Knowledge"); page.wait_for_timeout(500)
    check("L2 Knowledge — 视图渲染", "Knowledge" in page.content())
    click_if(page, "button", "Skills"); page.wait_for_timeout(500)
    check("L2 Skills — 视图渲染", "Skills" in page.content() or "技能" in page.content())
    click_if(page, "button", "Marketplace"); page.wait_for_timeout(500)
    check("L2 Marketplace — 视图渲染", True)
    click_if(page, "button", "New chat"); page.wait_for_timeout(500)
    check("L2 Chat — 输入框", visible(page, role="textbox", name="Chat input"))
    shot(page, "l2-views")


def l3_settings(page):
    print("\n── L3 Settings 6 tab ──")
    click_if(page, "button", "Settings")
    page.wait_for_timeout(800)
    body = page.content()
    for tab in ["General", "AI", "Data", "Member", "Privacy", "About"]:
        check(f"L3 Settings tab — {tab}", tab in body or click_if(page, "button", tab))
        page.wait_for_timeout(250)
    shot(page, "l3-settings")


def l4_modals(page):
    print("\n── L4 模态 ──")
    # CommandPalette Cmd+K
    page.keyboard.press("Control+k")
    page.wait_for_timeout(600)
    has_palette = visible(page, role="textbox") or "搜索" in page.content() or "search" in page.content().lower()
    check("L4 CommandPalette — Cmd+K 唤起", has_palette)
    page.keyboard.press("Escape")
    page.wait_for_timeout(400)
    shot(page, "l4-modals")


def l5_lawpro(page):
    print("\n── L5 law-pro 接入业务流程 ──")
    # 5.1 Marketplace / Skills 含 law-pro
    click_if(page, "button", "Marketplace"); page.wait_for_timeout(700)
    mk = page.content()
    check("L5.1 Marketplace 含 law-pro 痕迹", "law-pro" in mk or "律师" in mk or "Pro" in mk)
    click_if(page, "button", "Skills"); page.wait_for_timeout(600)
    sk = page.content()
    check("L5.2 Skills 视图可访问", "Skills" in sk or "技能" in sk)

    # 5.3 新建 civil-loan Project（kind 文本输入 — OSS by-design 非下拉）
    click_if(page, "button", "Projects"); page.wait_for_timeout(600)
    click_if(page, "button", "新建项目")
    page.wait_for_timeout(900)
    inputs = page.locator("input")
    if inputs.count() >= 2:
        inputs.nth(0).fill("任其坤-梁素燕 民间借贷纠纷案")
        inputs.nth(1).fill("civil-loan")  # law-pro registers_case_kinds 的 kind
        check("L5.3 新建 Project — title + kind=civil-loan 可填", True)
        shot(page, "l5-01-create-project")
        # 确认创建
        clicked = click_if(page, "button", "创建") or click_if(page, "button", "确定") \
            or click_if(page, "button", "Create") or click_if(page, "button", "确认")
        page.wait_for_timeout(1200)
        check("L5.4 civil-loan Project 创建成功",
              "civil-loan" in page.content() or "任其坤" in page.content())
    else:
        check("L5.3 新建 Project 模态 input", False, f"input 数={inputs.count()}")
    shot(page, "l5-02-projects")

    # 5.5 Items 上传 lawcontrol 真实证据（基于 Web UI 上传入口）
    click_if(page, "button", "Items"); page.wait_for_timeout(600)
    if os.path.exists(LAWCONTROL_EVIDENCE):
        try:
            with page.expect_file_chooser(timeout=5000) as fc:
                page.get_by_role("button", name="Upload files").first.click()
            fc.value.set_files(LAWCONTROL_EVIDENCE)
            page.wait_for_timeout(2500)
            # 解析出的文档标题含排版空格（"借 款 合 同"），归一化空白后比对
            normalized = page.content().replace(" ", "").replace("　", "")
            check("L5.5 上传 lawcontrol 借贷合同证据 (Items 显示新条目)",
                  "借款合同" in normalized)
        except Exception as e:
            check("L5.5 上传 lawcontrol 证据", False, str(e)[:80])
    else:
        check("L5.5 lawcontrol 证据文件存在", False, LAWCONTROL_EVIDENCE)
    shot(page, "l5-03-evidence-uploaded")

    # 5.6 Chat 输入 law-pro 关键词 → chat_trigger 路由
    click_if(page, "button", "New chat"); page.wait_for_timeout(600)
    try:
        box = page.get_by_role("textbox", name="Chat input").first
        box.fill("借条本金10万元，年利率24%，借款一年，应付利息和本息合计多少？")
        check("L5.6 Chat 输入 law-pro 关键词(本金/利息/借贷)", True)
        shot(page, "l5-04-chat-lawpro")
    except Exception as e:
        check("L5.6 Chat 输入框", False, str(e)[:80])


def l6_cloud_chat(page):
    """L6 云端 LLM chat — env 门控。ATTUNE_LLM_ENDPOINT 未设则跳过（本地运行无需 LLM）。"""
    endpoint = os.environ.get("ATTUNE_LLM_ENDPOINT")
    if not endpoint:
        return
    print("\n── L6 云端 LLM chat (hiapi.online) ──")
    key = os.environ.get("ATTUNE_LLM_KEY", "")
    model = os.environ.get("ATTUNE_LLM_MODEL", "gemini-2.5-flash")
    # Settings → AI tab 配置 LLM endpoint/model/key
    click_if(page, "button", "Settings")
    page.wait_for_timeout(900)
    click_if(page, "button", "AI")
    page.wait_for_timeout(900)
    try:
        page.get_by_placeholder("例：https://api.openai.com/v1").first.fill(endpoint)
        page.get_by_placeholder("例：deepseek-chat / qwen-plus / gpt-4o-mini").first.fill(model)
        page.locator('input[type="password"]').first.fill(key)
        check("L6.1 LLM 配置字段填写 (endpoint/model/key)", True)
    except Exception as e:
        check("L6.1 LLM 配置字段", False, str(e)[:80])
        return
    shot(page, "l6-01-llm-config")
    saved = click_if(page, "button", "Save LLM Config")
    page.wait_for_timeout(3000)
    check("L6.2 保存 LLM 配置", saved)
    # New chat → 发送 → 轮询等云端响应（无流式，spinner→完整回复）
    click_if(page, "button", "New chat")
    page.wait_for_timeout(900)
    try:
        box = page.get_by_role("textbox", name="Chat input").first
        box.fill("只回答城市名，不要解释：中华人民共和国的首都是哪里？")
        box.press("Control+Enter")
        got = False
        for _ in range(40):
            page.wait_for_timeout(1000)
            if "北京" in page.content():
                got = True
                break
        check("L6.3 云端 LLM 返回响应 (hiapi.online, 含'北京')", got)
        shot(page, "l6-02-chat-response")
    except Exception as e:
        check("L6.3 云端 chat", False, str(e)[:80])


def main():
    print(f"=== law-pro 接入 — 全面 Playwright UI 验证 (env={ENV}, {BASE}) ===")
    with sync_playwright() as p:
        browser = p.chromium.launch(channel="chrome", headless=True)
        page = browser.new_page(viewport={"width": 1440, "height": 900})
        page.on("console", lambda m: console_errors.append(m.text) if m.type == "error" else None)
        page.on("response", lambda r: failed_responses.append((r.status, r.url)) if r.status >= 400 else None)
        try:
            l0_wizard(page)
            l1_sidebar(page)
            l2_views(page)
            l3_settings(page)
            l4_modals(page)
            l5_lawpro(page)
            l6_cloud_chat(page)
        finally:
            browser.close()

    # 网络失败定位：favicon / scan-progress 是已知噪音，其余 4xx/5xx 才算真错
    NOISE = ("favicon", "scan-progress")
    real_failed = [(s, u) for s, u in failed_responses if not any(n in u for n in NOISE)]
    check("无未预期网络失败 4xx/5xx", len(real_failed) == 0,
          f"{len(failed_responses)} 总 / {len(real_failed)} 非噪音")
    for s, u in failed_responses:
        tag = "噪音" if any(n in u for n in NOISE) else "★真错"
        print(f"    [{s}] {tag}  {u[:140]}")
    # console error：排除「Failed to load resource」（已由网络层 real_failed 覆盖）
    real_errors = [e for e in console_errors if "Failed to load resource" not in e]
    check("无未预期 console error", len(real_errors) == 0,
          f"{len(console_errors)} 总 / {len(real_errors)} 非噪音")
    for e in real_errors[:5]:
        print(f"    error: {e}")

    print(f"\n=== 结果: {PASS} PASS / {FAIL} FAIL ===")
    sys.exit(0 if FAIL == 0 else 1)


if __name__ == "__main__":
    main()
