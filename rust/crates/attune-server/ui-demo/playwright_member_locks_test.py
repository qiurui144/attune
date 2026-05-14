"""Playwright e2e — member-settings.html 灰显验证.

启 attune-server + ui-demo HTTP → 打开 member-settings.html →
按钮触发不同 tier → 验证 input.disabled + badge.textContent.
"""

import asyncio
import sys
from playwright.async_api import async_playwright


async def main():
    issues = []
    async with async_playwright() as p:
        browser = await p.chromium.launch(channel="chrome", headless=True)
        page = await browser.new_page()
        page.set_default_timeout(8000)

        await page.goto("http://127.0.0.1:18921/member-settings.html")
        await page.locator("#server").fill("http://127.0.0.1:18920")

        # 1. 初始 loadLocks 默认 LoggedOut — 全 editable
        await page.get_by_role("button", name="1. 拉 locks").click()
        await page.wait_for_timeout(800)
        # llm_endpoint badge 应 hidden
        badge_class = await page.locator("#badge-llm_endpoint").get_attribute("class")
        if "hidden" not in (badge_class or ""):
            issues.append(f"LoggedOut llm_endpoint badge should be hidden, got class={badge_class}")
        disabled = await page.locator("#setting-llm_endpoint").is_disabled()
        if disabled:
            issues.append("LoggedOut: llm_endpoint should be editable")
        print(f"✓ LoggedOut: llm_endpoint disabled={disabled}, badge class={badge_class}")

        # 2. 切到付费会员
        await page.get_by_role("button", name="3. 模拟付费").click()
        await page.wait_for_timeout(800)
        badge_class = await page.locator("#badge-llm_endpoint").get_attribute("class")
        if "locked" not in (badge_class or ""):
            issues.append(f"Member: llm_endpoint should be locked, got class={badge_class}")
        disabled = await page.locator("#setting-llm_endpoint").is_disabled()
        if not disabled:
            issues.append("Member: llm_endpoint should be disabled")
        print(f"✓ Member: llm_endpoint disabled={disabled}, badge class={badge_class}")

        # local_folder_links 仍可改 (用户隐私)
        disabled = await page.locator("#setting-local_folder_links").is_disabled()
        if disabled:
            issues.append("Member: local_folder_links 应可改 (隐私)")
        print(f"✓ Member: local_folder_links 仍可改 (隐私 disabled={disabled})")

        # plugin_install 锁
        disabled = await page.locator("#setting-plugin_install").is_disabled()
        if not disabled:
            issues.append("Member: plugin_install should be locked")

        # 3. 切到企业
        await page.get_by_role("button", name="4. 模拟企业").click()
        await page.wait_for_timeout(800)
        badge_class = await page.locator("#badge-llm_endpoint").get_attribute("class")
        if "admin-only" not in (badge_class or ""):
            issues.append(f"Enterprise: llm_endpoint should be admin-only, got class={badge_class}")
        badge_text = await page.locator("#badge-llm_endpoint").text_content()
        if "管理员" not in (badge_text or ""):
            issues.append(f"Enterprise: badge text 应含'管理员', got: {badge_text}")
        print(f"✓ Enterprise: llm_endpoint badge='{badge_text}'")

        # 4. 登出
        await page.get_by_role("button", name="登出").click()
        await page.wait_for_timeout(800)
        disabled = await page.locator("#setting-llm_endpoint").is_disabled()
        if disabled:
            issues.append("After logout: llm_endpoint should be editable again")
        print(f"✓ Logged out: llm_endpoint disabled={disabled}")

        # 截图
        await page.screenshot(path="/tmp/member-settings-screenshot.png", full_page=True)
        print("✓ 截图: /tmp/member-settings-screenshot.png")

        # 检查 console 错误
        await browser.close()

    if issues:
        print("\n❌ 问题:")
        for i in issues:
            print(f"  - {i}")
        sys.exit(1)
    print("\n✓ Member locks Playwright e2e 全过")
    sys.exit(0)


if __name__ == "__main__":
    asyncio.run(main())
