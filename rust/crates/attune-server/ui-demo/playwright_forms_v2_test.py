import asyncio
import sys
from playwright.async_api import async_playwright


async def main():
    issues, insights = [], []
    async with async_playwright() as p:
        browser = await p.chromium.launch(channel="chrome", headless=True)
        page = await browser.new_page()
        page.set_default_timeout(8000)

        await page.goto("http://127.0.0.1:18921/forms-iframe.html")
        await page.locator("#server").fill("http://127.0.0.1:18920")

        # 点 "加载表单" → 应触发 preflight, 检测 vault 状态
        await page.get_by_role("button", name="加载表单").click()
        await page.wait_for_timeout(2000)
        response_text = await page.locator("#response").inner_text()
        print(f"=== 加载表单后 response ===\n{response_text[:500]}\n")

        if "Preflight 失败" in response_text:
            print("✓ vault locked 时 preflight 拦住, 给出友好提示 (UX 改进生效)")
        else:
            issues.append("preflight 未触发 (vault locked 时应该拦住)")

        # 检测 iframe 不应加载 raw JSON 错误 (应是 about:blank)
        iframe_src = await page.locator("#plugin-form").get_attribute("src")
        print(f"iframe src: {iframe_src}")
        if iframe_src and "about:blank" not in iframe_src and "/forms/" in iframe_src:
            issues.append(f"iframe 仍设置了 forms URL (应该 about:blank 阻止加载 raw JSON): {iframe_src}")
        else:
            print("✓ iframe 未加载 raw JSON (about:blank)")

        # 测试 POST submit
        await page.get_by_role("button", name="测试 POST /submit").click()
        await page.wait_for_timeout(2000)
        response_text = await page.locator("#response").inner_text()
        print(f"\n=== POST submit 后 response ===\n{response_text[:500]}")

        if "提交跳过" in response_text or "Preflight 失败" in response_text:
            print("✓ POST submit 也走 preflight, 提示用户 vault locked")
        else:
            insights.append("POST submit 未走 preflight (可能直接 fetch 拿到 403)")

        # 截图
        await page.screenshot(path="/tmp/forms-iframe-v2.png", full_page=True)
        print("\n截图: /tmp/forms-iframe-v2.png")

        await browser.close()

    if issues:
        print("\n❌ 问题:", *issues, sep="\n  - ")
        sys.exit(1)
    if insights:
        print("\n💡 反馈:", *insights, sep="\n  - ")
    print("\n✓ UX 改进验证通过")
    sys.exit(0)


asyncio.run(main())
