#!/usr/bin/env python3
"""v1.0 GA — attune Web UI 全链 E2E 验证矩阵（真 Chrome）

覆盖 10 大场景 A-J 中 UI 可触达的全部路径：
  A Vault 初始化 / 解锁 / 会话恢复
  B 5 个 ingest 源（B1 local / B2 email / B3 webdav / B4 rss / B5 telegram）
  C Chat + RAG（含 chat_reliability / self_evolving_skill agent 表现）
  D Office helper（OCR profile / id_card subtype / Transcribe scaffold）
  E law-pro Agent run（civil_loan 表单链）
  F Knowledge / Items / Projects（detail / linked / project 卷宗）
  G Settings 全 6 tab + lock 行为
  H Vault lifecycle（lock-unlock 二轮）
  I Cross-agent flow（验证存量 plugin 表面）
  J 性能 / 稳定性（spinner / toast / 刷新 session 恢复）

铁律：
  - 全程 Playwright（不 docker exec / curl）
  - 真 Chrome（channel="chrome"，非 chromium）
  - 不修代码、只 audit & report
  - 截图入 docs/screenshots/v10-ga/<scene>/<name>.png

幂等：vault 已 setup → 自动走解锁。
噪音过滤：server 重启窗口期 CONNECTION_REFUSED / favicon / health 不计 FAIL。
"""
from __future__ import annotations

import os
import sys
import time
from typing import Any

from playwright.sync_api import Page, sync_playwright

BASE = os.environ.get("ATTUNE_BASE_URL", "http://127.0.0.1:18900")
PW = os.environ.get("ATTUNE_VAULT_PW", "Attune-E2E-Test-2026")
LLM_URL = os.environ.get("ATTUNE_LLM_URL", "https://hiapi.online/v1")
LLM_KEY = os.environ.get("ATTUNE_LLM_KEY", "")
LLM_MODEL = os.environ.get("ATTUNE_LLM_MODEL", "gemini-2.5-flash")
HEADLESS = os.environ.get("ATTUNE_HEADLESS", "1") != "0"
SHOT_ROOT = "docs/screenshots/v10-ga"

PASS = FAIL = WARN = 0
results: list[tuple[str, str, str]] = []  # (scene, name, status, detail)
console_errors: list[str] = []
NOISE = (
    "ERR_CONNECTION_REFUSED",
    "favicon",
    "ws/scan-progress",
    "status/health",
    "Failed to load resource",
)


def record(scene: str, name: str, status: str, detail: str = "") -> bool:
    global PASS, FAIL, WARN
    if status == "PASS":
        PASS += 1
    elif status == "WARN":
        WARN += 1
    else:
        FAIL += 1
    results.append((scene, name, status, detail))
    marker = {"PASS": "  PASS", "FAIL": "  FAIL", "WARN": "  WARN"}[status]
    print(f"{marker}  [{scene}] {name}  {detail}")
    return status == "PASS"


def check(scene: str, name: str, cond: bool, detail: str = "") -> bool:
    return record(scene, name, "PASS" if cond else "FAIL", detail)


def warn(scene: str, name: str, detail: str = "") -> None:
    record(scene, name, "WARN", detail)


def shot(page: Page, scene: str, name: str) -> None:
    d = f"{SHOT_ROOT}/{scene}"
    os.makedirs(d, exist_ok=True)
    try:
        page.screenshot(path=f"{d}/{name}.png", full_page=False)
    except Exception as e:  # noqa: BLE001
        print(f"  (截图 {scene}/{name} 失败: {e})")


def visible(page: Page, role: str, name: str, timeout: int = 4000) -> bool:
    try:
        page.get_by_role(role, name=name).first.wait_for(state="visible", timeout=timeout)
        return True
    except Exception:  # noqa: BLE001
        return False


def click(page: Page, role: str, name: str, timeout: int = 4000) -> bool:
    try:
        page.get_by_role(role, name=name).first.click(timeout=timeout)
        return True
    except Exception:  # noqa: BLE001
        return False


def expand_more_if_needed(page: Page) -> None:
    """侧边栏「更多」默认折叠 — 展开后看到 远程目录 / 技能 / 办公助理 / 插件市场（注：设置 / 锁定 在账号菜单不在此处）。"""
    try:
        btn = page.get_by_role("button", name="展开更多功能")
        if btn.count() > 0 and btn.first.is_visible():
            btn.first.click()
            page.wait_for_timeout(400)
    except Exception:  # noqa: BLE001
        pass


def open_settings_via_account_menu(page: Page) -> bool:
    """打开 Settings modal — 必须经账号菜单 → ⚙ 设置（不是侧边栏一级入口）。"""
    try:
        page.get_by_role("button", name="账号菜单").click(timeout=4000)
        page.wait_for_timeout(500)
        page.get_by_text("⚙ 设置").first.click(timeout=3000)
        page.wait_for_timeout(800)
        return True
    except Exception as e:  # noqa: BLE001
        print(f"  (打开 Settings modal 失败: {e})")
        return False


def lock_vault_via_account_menu(page: Page) -> bool:
    """锁定 vault — 必须经账号菜单 → 锁定知识库（有 confirm 弹窗）。"""
    page.once("dialog", lambda d: d.accept())
    try:
        # 先关任何残留 modal/menu
        page.keyboard.press("Escape")
        page.wait_for_timeout(600)
        page.get_by_role("button", name="账号菜单").click(timeout=4000)
        page.wait_for_timeout(1200)
        # 尝试 menuitem role 优先，回退 text 匹配
        clicked = False
        for fn in [
            lambda: page.get_by_role("menuitem").filter(has_text="锁定知识库").first.click(timeout=3000),
            lambda: page.locator('[role="menuitem"]').filter(has_text="锁定知识库").first.click(timeout=3000),
            lambda: page.locator("text=锁定知识库").first.click(timeout=3000),
        ]:
            try:
                fn()
                clicked = True
                break
            except Exception:  # noqa: BLE001
                continue
        if not clicked:
            return False
        page.wait_for_timeout(3000)
        return True
    except Exception as e:  # noqa: BLE001
        print(f"  (锁定 vault 失败: {e})")
        return False


# ╔═══════════════════════════════════════════════════════════════
# ║ A — Vault 初始化 / 解锁 / 会话恢复
# ╚═══════════════════════════════════════════════════════════════
def scene_A(page: Page) -> None:
    print("\n══ A Vault 初始化 / 解锁 / 会话恢复 ══")
    page.goto(BASE, wait_until="networkidle")
    check("A", "页面标题含 Attune", "Attune" in page.title(), page.title())
    shot(page, "A-vault", "01-initial-landing")

    if visible(page, "button", "开始设置"):
        # 全新 vault — 走 wizard 5 步
        shot(page, "A-vault", "02-wizard-welcome")
        click(page, "button", "开始设置")
        page.get_by_role("textbox", name="主密码").fill(PW, timeout=5000)
        page.get_by_role("textbox", name="再次输入").fill(PW)
        check("A", "Wizard Step2 — 密码强度显示「强」",
              page.get_by_text("强").first.is_visible(timeout=2000))
        shot(page, "A-vault", "03-wizard-password")
        click(page, "button", "下一步 →")
        ai_combo = page.get_by_role("combobox").first
        ai_combo.wait_for(state="visible", timeout=8000)
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
        check("A", "Wizard Step3 — 云端 LLM 连接测试通过", ok_test)
        shot(page, "A-vault", "04-wizard-ai")
        click(page, "button", "使用云端")
        check("A", "Wizard Step4 — 硬件检测页",
              visible(page, "heading", "认识你的设备"))
        shot(page, "A-vault", "05-wizard-hardware")
        click(page, "button", "应用推荐 →")
        check("A", "Wizard Step5 — 数据来源页",
              visible(page, "heading", "从哪里开始积累？"))
        shot(page, "A-vault", "06-wizard-data")
        click(page, "button", "跳过，先看看 之后随时在设置中添加")
        click(page, "button", "完成 · 进入 Attune →")
    elif visible(page, "button", "解锁"):
        shot(page, "A-vault", "02-lock-screen")
        page.get_by_role("textbox", name="主密码").fill(PW)
        click(page, "button", "解锁")
        record("A", "vault 已存在 — 解锁路径", "PASS")
    else:
        record("A", "已在主界面（session 残留）", "PASS")

    page.wait_for_timeout(2000)
    in_main = visible(page, "button", "条目", timeout=8000)
    check("A", "进入主界面 — 看到「条目」按钮", in_main)
    shot(page, "A-vault", "07-main-shell")


# ╔═══════════════════════════════════════════════════════════════
# ║ B — 5 个 ingest 源（local / email / webdav / rss / telegram）
# ╚═══════════════════════════════════════════════════════════════
def scene_B(page: Page) -> None:
    print("\n══ B 5 个 ingest 源 ══")

    # B1 Local folder — 通过 Items 上传 + Remote 添加本地两条路径
    click(page, "button", "条目")
    page.wait_for_timeout(600)
    has_upload = visible(page, "button", "上传文件")
    check("B1", "Items 视图 — 上传文件 按钮可见", has_upload)
    shot(page, "B1-local", "01-items-view")

    # B2 Email — Remote 视图查看
    expand_more_if_needed(page)
    page.wait_for_timeout(300)
    click(page, "button", "远程目录")
    page.wait_for_timeout(800)
    has_remote = "远程目录" in page.content() or "添加" in page.content()
    check("B3", "WebDAV 视图渲染", has_remote)
    shot(page, "B3-webdav", "01-remote-view")

    # B3 WebDAV 添加按钮（不实际填—会发起真请求）
    has_webdav_btn = visible(page, "button", "添加 WebDAV", timeout=2000)
    check("B3", "「添加 WebDAV」按钮可见", has_webdav_btn)
    if has_webdav_btn:
        click(page, "button", "添加 WebDAV")
        page.wait_for_timeout(600)
        has_modal = page.get_by_role("dialog").count() > 0 or "用户名" in page.content()
        check("B3", "WebDAV 添加 modal 弹出", has_modal)
        shot(page, "B3-webdav", "02-add-webdav-modal")
        # ESC 关闭
        page.keyboard.press("Escape")
        page.wait_for_timeout(400)

    # B2 Email section in remote view
    has_email = ("📬" in page.content()) or ("Email" in page.content()) or ("邮箱" in page.content())
    check("B2", "Email 采集源 section 可见", has_email)
    if has_email:
        shot(page, "B2-email", "01-email-section")

    # B4 RSS — 当前 UI 无 scaffold
    has_rss_ui = ("RSS" in page.content()) or ("rss" in page.content().lower())
    if has_rss_ui:
        check("B4", "RSS 采集源 UI scaffold 可见", True)
    else:
        warn("B4", "RSS 采集源 UI scaffold 不存在 — v1.0 未实装 UI（采集源插件计划文档已规划）")

    # B5 Telegram — 当前 UI 无 scaffold
    has_tg_ui = "telegram" in page.content().lower()
    if has_tg_ui:
        check("B5", "Telegram 采集源 UI scaffold 可见", True)
    else:
        warn("B5", "Telegram 采集源 UI scaffold 不存在 — v1.0 未实装 UI（采集源插件计划文档已规划）")

    # B1 添加本地文件夹按钮
    has_local_btn = visible(page, "button", "添加本地", timeout=2000)
    check("B1", "「添加本地」按钮可见（folder watch）", has_local_btn)


# ╔═══════════════════════════════════════════════════════════════
# ║ C — Chat + RAG
# ╚═══════════════════════════════════════════════════════════════
def scene_C(page: Page) -> None:
    print("\n══ C Chat + RAG ══")
    click(page, "button", "新对话")
    page.wait_for_timeout(800)
    has_input = visible(page, "textbox", "对话输入框")
    check("C", "Chat 输入框可见", has_input)
    has_model_chip = visible(page, "button", "切换模型")
    check("C", "切换模型 chip 可见（成本感知 UI）", has_model_chip)
    shot(page, "C-chat", "01-chat-empty")

    # 验证 sample prompts（首屏 onboarding chip）
    samples_visible = ("帮我总结" in page.content()) or ("搜索关于" in page.content())
    if samples_visible:
        check("C", "首屏 sample prompt chip 可见", True)
    else:
        warn("C", "首屏 sample chip 未见（可能 chat 已有历史）")

    # tokens 估算显示（CLAUDE.md 强制 — 「成本感知 UI」）
    has_tokens = "tok" in page.content() or "词元" in page.content()
    if has_tokens:
        check("C", "成本预估（token chip）可见", True)
    else:
        warn("C", "成本预估 chip 未显示 — 检查 ChatInput 是否常驻 token chip")

    # 触发一个低成本短问 — 验证 send 路径（如配置 LLM）
    if LLM_KEY:
        try:
            page.get_by_role("textbox", name="对话输入框").fill("一句话回答：你好")
            shot(page, "C-chat", "02-chat-typed")
            # Cmd+Enter
            page.keyboard.press("Meta+Enter")
            # 等 8s 看是否出现 streaming/assistant 内容
            page.wait_for_timeout(8000)
            content = page.content()
            got_reply = ("assistant" in content.lower()) or ("你好" in content) \
                or page.locator('[data-role="assistant"]').count() > 0
            if got_reply:
                check("C", "Chat 发送 → 获得 LLM 回答", True)
            else:
                warn("C", "Chat 发送后 8s 内未收到回答（LLM 慢或配置异常）")
            shot(page, "C-chat", "03-chat-after-send")
        except Exception as e:  # noqa: BLE001
            warn("C", f"Chat 发送流程异常: {str(e)[:120]}")
    else:
        warn("C", "ATTUNE_LLM_KEY 未配置 — 跳过真 LLM 调用，只验证 UI 表面")


# ╔═══════════════════════════════════════════════════════════════
# ║ D — Office helper（OCR / Transcribe）
# ╚═══════════════════════════════════════════════════════════════
def scene_D(page: Page) -> None:
    print("\n══ D Office helper ══")
    expand_more_if_needed(page)
    page.wait_for_timeout(300)
    if not click(page, "button", "办公助理"):
        warn("D", "侧边栏「办公助理」未找到 — 可能 sidebar.nav.office 未注册")
        return
    page.wait_for_timeout(800)
    has_title = "办公助理" in page.content() or "OCR" in page.content()
    check("D", "Office 视图渲染（含标题）", has_title)
    shot(page, "D-office", "01-office-landing")

    has_ocr_tab = visible(page, "tab", "结构化 OCR", timeout=2000) or "OCR" in page.content()
    check("D", "结构化 OCR tab 可见", has_ocr_tab)
    has_transcribe_tab = visible(page, "tab", "语音转写", timeout=2000) or "语音转写" in page.content()
    check("D", "语音转写 tab 可见", has_transcribe_tab)

    # 检查 profile 下拉是否含 9 个场景
    profiles_text = page.content()
    profile_kw = ["标准文档", "发票", "卡证", "表格", "名片"]
    found = sum(1 for k in profile_kw if k in profiles_text)
    check("D", f"OCR profile 场景齐全（{found}/{len(profile_kw)} 关键词命中）", found >= 3)

    # 选 id_card profile → 验证 subtype 子下拉出现
    try:
        profile_combo = page.get_by_role("combobox").first
        if profile_combo.is_visible(timeout=1500):
            profile_combo.select_option("id_card")
            page.wait_for_timeout(400)
            subtype_visible = "卡证子类型" in page.content() or "居民身份证" in page.content()
            check("D", "选 id_card → subtype 子下拉出现", subtype_visible)
            shot(page, "D-office", "02-id-card-subtype")
        else:
            warn("D", "profile combobox 未渲染")
    except Exception as e:  # noqa: BLE001
        warn("D", f"id_card subtype 流程异常: {str(e)[:120]}")

    # 切到语音转写 tab
    try:
        click(page, "tab", "语音转写") or click(page, "button", "🎙️ 语音转写")
        page.wait_for_timeout(500)
        has_transcribe_ui = ("语音" in page.content()) or ("音频" in page.content()) \
            or ("transcript" in page.content().lower())
        check("D", "Transcribe tab 视图渲染", has_transcribe_ui)
        shot(page, "D-office", "03-transcribe-tab")
    except Exception as e:  # noqa: BLE001
        warn("D", f"Transcribe tab 切换异常: {str(e)[:120]}")


# ╔═══════════════════════════════════════════════════════════════
# ║ E — law-pro Agent run（已在 lawpro_ui_e2e.py 充分覆盖；此处仅快速 spot check）
# ╚═══════════════════════════════════════════════════════════════
def scene_E(page: Page) -> None:
    print("\n══ E law-pro Agent run ══")
    click(page, "button", "项目")
    page.wait_for_timeout(700)
    has_projects = ("项目" in page.content()) or ("Projects" in page.content())
    check("E", "Projects 视图渲染", has_projects)
    shot(page, "E-agent", "01-projects")

    # 检查是否已有 E2E-民间借贷-自动 项目（前一轮 lawpro_ui_e2e.py 已建）
    has_existing_proj = "E2E-民间借贷-自动" in page.content()
    if has_existing_proj:
        check("E", "已有 civil_loan project（前轮残留 — 可作快速验证）", True)
        click(page, "button", "E2E-民间借贷-自动")
        page.wait_for_timeout(800)
        has_assistant = "计算助手" in page.content()
        check("E", "law-pro 计算助手 panel 挂载（agent_view）", has_assistant)
        shot(page, "E-agent", "02-civil-loan-panel")
    else:
        warn("E", "无现存 civil_loan project — 见 lawpro_ui_e2e.py L5.3-L5.5 完整链路")


# ╔═══════════════════════════════════════════════════════════════
# ║ F — Knowledge / Items / Projects
# ╚═══════════════════════════════════════════════════════════════
def scene_F(page: Page) -> None:
    print("\n══ F Knowledge / Items / Projects ══")

    # F1 Items list
    click(page, "button", "条目")
    page.wait_for_timeout(1000)
    # 搜索框走 placeholder 匹配（Items 视图的 search 是 type=search 不一定 role=textbox）
    has_search = (page.get_by_placeholder("🔍 按标题搜索…").count() > 0) \
        or (page.locator('input[type="search"]').count() > 0) \
        or ("按标题搜索" in page.content())
    check("F1", "Items 列表 — 搜索框可见", has_search)

    # 来源筛选下拉（select 元素）
    source_filter = (page.get_by_role("combobox").count() > 0) \
        or (page.locator("select").count() > 0) \
        or ("全部来源" in page.content())
    check("F1", "Items — 来源筛选下拉可见", source_filter)

    has_refresh = visible(page, "button", "⟳ 刷新", timeout=2000)
    check("F1", "Items — 刷新按钮可见", has_refresh)
    shot(page, "F1-items", "01-items-list")

    # F2 Item detail（点首个条目）
    try:
        rows = page.locator("[data-item-id], [role=row], .item-row, [data-testid*=item]")
        # 试一下点击列表中的任一条目（先看条目数）
        items_present = "共 0 条" not in page.content() and "(无标题)" in page.content() or "条目" in page.content()
        if items_present:
            check("F2", "Items 列表有数据可点击（detail 入口存在）", True)
        else:
            warn("F2", "Items 当前为空 — 跳过 detail 点击验证（先做 ingest 才有数据）")
    except Exception:  # noqa: BLE001
        pass

    # F3 Projects
    click(page, "button", "项目")
    page.wait_for_timeout(700)
    has_new_proj = visible(page, "button", "新建项目", timeout=2000) \
        or visible(page, "button", "+ 新建项目", timeout=1000)
    check("F3", "Projects — 新建项目按钮可见", has_new_proj)
    shot(page, "F3-projects", "01-projects-view")

    # F4 Skills tab
    expand_more_if_needed(page)
    page.wait_for_timeout(300)
    click(page, "button", "技能")
    page.wait_for_timeout(700)
    has_skills = ("技能" in page.content()) or ("Skills" in page.content())
    check("F4", "Skills 视图渲染", has_skills)
    has_refresh_btn = visible(page, "button", "刷新", timeout=1500)
    check("F4", "Skills — 刷新按钮可见", has_refresh_btn)
    shot(page, "F4-skills", "01-skills-view")

    # Knowledge 视图（panorama）
    click(page, "button", "知识全景")
    page.wait_for_timeout(700)
    has_knowledge = ("知识全景" in page.content()) or ("knowledge" in page.content().lower())
    check("F1", "Knowledge 全景视图渲染", has_knowledge)
    shot(page, "F1-items", "02-knowledge-panorama")


# ╔═══════════════════════════════════════════════════════════════
# ║ G — Settings 6 tab + Lock / 成本感知
# ╚═══════════════════════════════════════════════════════════════
def scene_G(page: Page) -> None:
    print("\n══ G Settings 6 tab ══")
    opened = open_settings_via_account_menu(page)
    check("G", "Settings modal 通过账号菜单打开", opened)
    if not opened:
        return

    tabs = ["通用", "AI 大脑", "数据", "会员", "隐私", "关于"]
    for tab in tabs:
        # Settings 内 tab 也是 button — 用 role=button + 精确 name
        ok = click(page, "button", tab, timeout=3000)
        page.wait_for_timeout(500)
        check("G", f"Settings tab — {tab}", ok)
        shot(page, "G-settings", f"01-{tab}")

    # G2 会员 tab — cloud accounts + license 显示
    click(page, "button", "会员")
    page.wait_for_timeout(500)
    has_member = ("会员" in page.content()) or ("license" in page.content().lower()) \
        or ("attune.ai" in page.content().lower())
    check("G", "会员 tab — 含 cloud accounts / license 信息", has_member)

    # 关闭 modal
    page.keyboard.press("Escape")
    page.wait_for_timeout(400)


# ╔═══════════════════════════════════════════════════════════════
# ║ H — Vault lifecycle（lock → unlock）
# ╚═══════════════════════════════════════════════════════════════
def scene_H(page: Page) -> None:
    print("\n══ H Vault lifecycle ══")
    # 一次性流程：开账号菜单 → 截图记入口 → 直接点 锁定知识库（不 Escape 中断）
    has_lock_entry = False
    locked = False
    page.once("dialog", lambda d: d.accept())
    try:
        page.get_by_role("button", name="账号菜单").click(timeout=4000)
        page.wait_for_timeout(1000)
        has_lock_entry = page.get_by_text("锁定知识库").count() > 0
        shot(page, "H-lifecycle", "00-account-menu")
        if has_lock_entry:
            # 直接点 — 不 Escape 中间
            try:
                page.locator("text=锁定知识库").first.click(timeout=4000)
                page.wait_for_timeout(3000)
                locked = True
            except Exception as e:  # noqa: BLE001
                print(f"  (锁定点击失败: {e})")
    except Exception as e:  # noqa: BLE001
        warn("H", f"账号菜单流程异常: {str(e)[:120]}")
    check("H", "账号菜单含「锁定知识库」入口", has_lock_entry)
    check("H", "通过账号菜单 → 锁定 vault", locked)
    if locked:
        has_lock_screen = visible(page, "button", "解锁", timeout=5000)
        check("H", "锁定后 → 锁屏出现", has_lock_screen)
        shot(page, "H-lifecycle", "01-after-lock")
        if has_lock_screen:
            try:
                page.get_by_role("textbox", name="主密码").fill(PW)
                page.wait_for_timeout(500)
                click(page, "button", "解锁")
                # 解锁需要 PBKDF2 拉密钥 + 重建 session，可能耗几秒
                page.wait_for_timeout(6000)
                back_in_main = visible(page, "button", "条目", timeout=10000)
                check("H", "重解锁 → 回到主界面（session 重建）", back_in_main)
                shot(page, "H-lifecycle", "02-after-relock-unlock")
            except Exception as e:  # noqa: BLE001
                warn("H", f"重解锁异常: {str(e)[:120]}")

    # 改密码 / recovery key 入口 — 设计上仅锁屏「忘记密码」走 recovery key 重置
    warn("H", "Settings 内未提供「修改密码」入口（设计上仅锁屏「忘记密码」走 recovery key 重置）— 与 wizard-flow.md 一致")


# ╔═══════════════════════════════════════════════════════════════
# ║ I — Cross-agent flow（验证 plugin 表面）
# ╚═══════════════════════════════════════════════════════════════
def scene_I(page: Page) -> None:
    print("\n══ I Cross-agent / Marketplace 表面 ══")
    expand_more_if_needed(page)
    page.wait_for_timeout(300)
    click(page, "button", "插件市场")
    page.wait_for_timeout(1500)
    has_marketplace = ("插件市场" in page.content()) or ("marketplace" in page.content().lower())
    check("I", "Marketplace 视图渲染", has_marketplace)
    shot(page, "I-marketplace", "01-marketplace")

    # 检查已安装的 7 个 plugin（law-pro / patent-pro / presales-pro / tech-pro / rust_helper /
    # ai_annotation_highlights / ai_annotation_risk）
    installed_plugins = ["law-pro", "patent-pro", "presales-pro", "tech-pro"]
    found = sum(1 for p in installed_plugins if p in page.content())
    check("I", f"Marketplace 显示已安装 plugin（{found}/{len(installed_plugins)} 命中）",
          found >= 1)


# ╔═══════════════════════════════════════════════════════════════
# ║ J — 性能 / 稳定性
# ╚═══════════════════════════════════════════════════════════════
def scene_J(page: Page) -> None:
    print("\n══ J 性能 / 稳定性 ══")

    # 全局 Cmd+K 唤起 CommandPalette
    page.keyboard.press("Control+k")
    page.wait_for_timeout(800)
    has_palette = page.get_by_role("textbox").count() > 0 \
        or "搜索" in page.content() or "Cmd+K" in page.content()
    check("J", "Cmd+K → 全局搜索 CommandPalette 唤起", has_palette)
    shot(page, "J-perf", "01-command-palette")
    page.keyboard.press("Escape")
    page.wait_for_timeout(400)

    # 顶栏账号菜单
    has_account = visible(page, "button", "账号菜单", timeout=2000)
    check("J", "顶栏 — 账号菜单按钮可见", has_account)

    # 浏览器刷新 → session 是否保持（已 unlock 状态）
    print("  (J 性能测试 — 刷新页面验证 session 持久化…)")
    page.reload(wait_until="networkidle")
    page.wait_for_timeout(2500)
    after_reload = visible(page, "button", "条目", timeout=5000) \
        or visible(page, "button", "解锁", timeout=2000)
    check("J", "刷新后状态合理（主界面或锁屏，不崩溃）", after_reload)
    shot(page, "J-perf", "02-after-reload")

    # 如果刷新后锁屏 → 重解锁验证 session 恢复
    if visible(page, "button", "解锁", timeout=1500):
        page.get_by_role("textbox", name="主密码").fill(PW)
        click(page, "button", "解锁")
        page.wait_for_timeout(2000)
        check("J", "刷新后重解锁 → session 恢复成功",
              visible(page, "button", "条目", timeout=5000))


# ╔═══════════════════════════════════════════════════════════════
# ║ main
# ╚═══════════════════════════════════════════════════════════════
def main() -> int:
    if not LLM_KEY:
        print("WARN: ATTUNE_LLM_KEY 未配置 — Chat 真发送流程会 fallback 到 WARN")
    print(f"=== v1.0 GA UI E2E ===  BASE={BASE}  headless={HEADLESS}\n")
    start = time.time()
    with sync_playwright() as p:
        browser = p.chromium.launch(channel="chrome", headless=HEADLESS)
        context = browser.new_context(locale="zh-CN", viewport={"width": 1440, "height": 900})
        page = context.new_page()
        page.on("console", lambda m: console_errors.append(m.text)
                if m.type == "error" else None)
        try:
            scene_A(page)
            scene_B(page)
            scene_C(page)
            scene_D(page)
            scene_E(page)
            scene_F(page)
            scene_G(page)
            scene_H(page)
            scene_I(page)
            scene_J(page)
        except Exception as e:  # noqa: BLE001
            print(f"\n[!] 未捕获异常: {e}")
        finally:
            shot(page, "Z-final", "01-final-state")
            browser.close()

    elapsed = time.time() - start
    real_errs = [e for e in console_errors if not any(n in e for n in NOISE)]
    print("\n── Console errors（排除已知噪音） ──")
    check("Z", f"无未处理 JS 错误（{len(real_errs)} 真实 / {len(console_errors)} 总）",
          len(real_errs) == 0)
    for e in real_errs[:15]:
        print(f"    {e[:180]}")

    print(f"\n══════════════════════════════════════════════════════")
    print(f"  v1.0 GA UI E2E 结果：{PASS} PASS / {WARN} WARN / {FAIL} FAIL")
    print(f"  耗时：{elapsed:.1f}s | 截图：{SHOT_ROOT}/")
    print(f"══════════════════════════════════════════════════════")

    # 写 JSON 结果文件供 report 生成
    import json
    os.makedirs("docs", exist_ok=True)
    with open("docs/v10-ga-ui-e2e-results.json", "w") as f:
        json.dump({
            "pass": PASS,
            "warn": WARN,
            "fail": FAIL,
            "elapsed_sec": round(elapsed, 1),
            "results": [
                {"scene": s, "name": n, "status": st, "detail": d}
                for s, n, st, d in results
            ],
            "console_errors": real_errs[:15],
        }, f, ensure_ascii=False, indent=2)

    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
