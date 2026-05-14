"""attune Web UI 端到端 Playwright 验证.

验证目标:
1. forms-iframe.html demo 能正常加载
2. 加载表单按钮 → iframe.src 设置成功
3. POST /submit 按钮 → fetch attune-server → 返回 JSON
4. 设计反馈: 整体布局合理性 / 用户操作链路顺畅
"""

import asyncio
import sys
from playwright.async_api import async_playwright


async def main():
    issues = []
    insights = []

    async with async_playwright() as p:
        browser = await p.chromium.launch(channel="chrome", headless=True)
        page = await browser.new_page()
        page.set_default_timeout(8000)

        # 收集 console 错误
        console_errors = []
        page.on("console", lambda msg: console_errors.append(msg.text) if msg.type == "error" else None)
        page.on("pageerror", lambda err: console_errors.append(f"pageerror: {err}"))

        # 1. 打开 demo
        await page.goto("http://127.0.0.1:18921/forms-iframe.html")
        title = await page.title()
        print(f"✓ 页面打开: title={title!r}")
        if title != "Attune Plugin Form Demo":
            issues.append(f"title mismatch: {title}")

        # 2. 检查关键控件
        for sel, name in [
            ("#server", "server URL input"),
            ("#plugin", "plugin id input"),
            ("#form", "form id input"),
            ("#plugin-form", "iframe"),
            ("#response", "response pre"),
        ]:
            cnt = await page.locator(sel).count()
            if cnt != 1:
                issues.append(f"控件缺失: {name} ({sel}), count={cnt}")
            else:
                print(f"✓ 控件存在: {name}")

        # 3. 默认 server URL
        server_val = await page.locator("#server").input_value()
        print(f"  默认 server URL: {server_val}")
        # 改为我们的 server
        await page.locator("#server").fill("http://127.0.0.1:18920")

        # 4. 点 "加载表单"
        await page.get_by_role("button", name="加载表单").click()
        await page.wait_for_timeout(500)
        iframe_src = await page.locator("#plugin-form").get_attribute("src")
        print(f"✓ iframe src 设置为: {iframe_src}")
        if "law-pro" not in (iframe_src or ""):
            issues.append("iframe src 未含 plugin id")

        # 5. iframe 加载结果 (plugin 未装载时应返 4xx, 但 iframe 仍 navigation)
        await page.wait_for_timeout(500)
        # 5b. 点 "测试 POST /submit"
        await page.get_by_role("button", name="测试 POST /submit").click()
        await page.wait_for_timeout(1500)
        response_text = await page.locator("#response").inner_text()
        print(f"✓ POST submit 响应:\n{response_text[:300]}")
        if "status:" not in response_text:
            issues.append("POST 响应未含 status 字段")
        # vault locked → 403 (路由注册成功)
        if "status: 403" in response_text or "status: 404" in response_text:
            print("  → 路由已注册 (vault locked / plugin 未装), 4xx 符合预期")
        else:
            insights.append(f"非 4xx 响应: {response_text[:100]}")

        # 6. 设计反馈
        # 6a. 关键控件 viewport 内
        page_height = await page.evaluate("document.body.scrollHeight")
        viewport_h = page.viewport_size["height"]
        if page_height > viewport_h * 2:
            insights.append(f"页面过长 ({page_height}px), 可能需要折叠/分 tab")

        # 6b. 默认 server URL 应该能改 (输入框可写)
        try:
            await page.locator("#server").fill("test")
            current = await page.locator("#server").input_value()
            if current != "test":
                issues.append("server URL input 不可编辑")
        except Exception as e:
            issues.append(f"server URL 输入异常: {e}")

        # 7. console 错误
        for err in console_errors:
            if "favicon" not in err.lower():  # 忽略 favicon 缺失
                insights.append(f"console 错误: {err[:100]}")

        # 8. 截图供人审
        await page.screenshot(path="/tmp/forms-iframe-screenshot.png", full_page=True)
        print("✓ 截图: /tmp/forms-iframe-screenshot.png")

        await browser.close()

    print("\n=== 验证结果 ===")
    if issues:
        print("❌ 问题:")
        for i in issues:
            print(f"  - {i}")
    else:
        print("✓ 无关键问题")

    if insights:
        print("💡 设计反馈:")
        for i in insights:
            print(f"  - {i}")

    sys.exit(1 if issues else 0)


if __name__ == "__main__":
    asyncio.run(main())
