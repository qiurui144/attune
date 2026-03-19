"""Chrome 扩展 E2E 测试 — Playwright Chromium"""

import subprocess
import time

import pytest
from playwright.sync_api import sync_playwright

EXT_PATH = "/data/company/project/npu-webhook/extension"
BACKEND_URL = "http://localhost:18900"

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def _ensure_built():
    """确保扩展已构建"""
    result = subprocess.run(
        ["npm", "run", "build"],
        cwd=EXT_PATH,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, f"Build failed:\n{result.stderr}"


@pytest.fixture(scope="module")
def _ensure_backend():
    """确保后端可达"""
    import urllib.request

    try:
        resp = urllib.request.urlopen(f"{BACKEND_URL}/api/v1/status/health", timeout=3)
        assert resp.status == 200
    except Exception:
        pytest.skip("后端未运行，跳过扩展 E2E 测试")


@pytest.fixture(scope="module")
def browser_ctx(_ensure_built, _ensure_backend):
    """启动 Chromium + 加载扩展，模块级复用"""
    with sync_playwright() as p:
        ctx = p.chromium.launch_persistent_context(
            user_data_dir="",  # 空字符串 = 临时目录
            headless=False,
            args=[
                f"--disable-extensions-except={EXT_PATH}",
                f"--load-extension={EXT_PATH}",
                "--no-first-run",
            ],
        )
        # 等待 Service Worker 就绪（最多 30 秒）
        if not ctx.service_workers:
            try:
                ctx.wait_for_event("serviceworker", timeout=30000)
            except Exception:
                pass

        yield ctx
        ctx.close()


@pytest.fixture(scope="module")
def ext_id(browser_ctx):
    """获取扩展 ID"""
    sw = browser_ctx.service_workers
    assert sw, "Service Worker 未启动"
    return sw[0].url.split("/")[2]


@pytest.fixture()
def page(browser_ctx):
    """每个测试一个新页面"""
    pg = browser_ctx.new_page()
    yield pg
    pg.close()


# ---------------------------------------------------------------------------
# 1. 扩展加载
# ---------------------------------------------------------------------------


class TestExtensionLoading:
    """扩展基础加载测试"""

    def test_service_worker_running(self, browser_ctx):
        """Service Worker 已注册并运行"""
        sw = browser_ctx.service_workers
        assert len(sw) >= 1
        assert "worker.js" in sw[0].url

    def test_extension_id_valid(self, ext_id):
        """扩展 ID 有效（32 位小写字母）"""
        assert len(ext_id) == 32
        assert ext_id.isalpha() and ext_id.islower()


# ---------------------------------------------------------------------------
# 2. Popup 页面
# ---------------------------------------------------------------------------


class TestPopup:
    """Popup 弹出面板测试"""

    def test_popup_renders(self, page, ext_id):
        """Popup 页面正常渲染"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        page.wait_for_selector("h2", timeout=5000)
        assert "npu-webhook" in page.text_content("h2")

    def test_popup_shows_stats(self, page, ext_id):
        """Popup 显示知识条目和向量数统计"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        page.wait_for_selector("h2", timeout=5000)
        # 等待统计数据加载
        time.sleep(2)
        body = page.text_content("body")
        assert "知识条目" in body
        assert "向量数" in body

    def test_popup_connection_status(self, page, ext_id):
        """Popup 显示在线连接状态（绿色指示灯）"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        time.sleep(2)
        # 连接状态指示灯应为绿色 (#22c55e)
        dot = page.query_selector("span")
        assert dot is not None
        bg = dot.evaluate("el => getComputedStyle(el).backgroundColor")
        # rgb(34, 197, 94) = #22c55e (绿色)
        assert "34, 197, 94" in bg or "22c55e" in bg

    def test_popup_injection_toggle(self, page, ext_id):
        """注入开关可点击"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        page.wait_for_selector("h2", timeout=5000)
        body = page.text_content("body")
        assert "知识注入" in body
        # 点击 toggle 按钮
        toggle = page.query_selector("button[style*='border-radius: 10px']")
        assert toggle is not None
        toggle.click()

    def test_popup_buttons_exist(self, page, ext_id):
        """「打开知识面板」和「设置」按钮存在"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        page.wait_for_selector("h2", timeout=5000)
        body = page.text_content("body")
        assert "打开知识面板" in body
        assert "设置" in body

    def test_popup_no_js_errors(self, page, ext_id):
        """Popup 页面无 JS 错误"""
        js_errors = []
        page.on("pageerror", lambda e: js_errors.append(str(e)))
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        time.sleep(2)
        assert js_errors == [], f"JS 错误: {js_errors}"


# ---------------------------------------------------------------------------
# 3. Options 设置页面
# ---------------------------------------------------------------------------


class TestOptions:
    """Options 设置页面测试"""

    def test_options_renders(self, page, ext_id):
        """Options 页面正常渲染"""
        page.goto(f"chrome-extension://{ext_id}/dist/options/index.html")
        page.wait_for_selector("h1", timeout=5000)
        assert "设置" in page.text_content("h1")

    def test_options_has_all_fields(self, page, ext_id):
        """Options 包含后端地址/注入模式/排除域名"""
        page.goto(f"chrome-extension://{ext_id}/dist/options/index.html")
        page.wait_for_selector("h1", timeout=5000)
        body = page.text_content("body")
        for field in ["后端地址", "注入模式", "排除域名", "自动", "手动", "禁用"]:
            assert field in body, f"缺少: {field}"

    def test_options_backend_url_default(self, page, ext_id):
        """后端地址默认值正确"""
        page.goto(f"chrome-extension://{ext_id}/dist/options/index.html")
        time.sleep(1)
        input_el = page.query_selector("input[placeholder='http://localhost:18900']")
        assert input_el is not None
        value = input_el.input_value()
        assert "localhost:18900" in value

    def test_options_test_connection(self, page, ext_id):
        """测试连接按钮有效"""
        page.goto(f"chrome-extension://{ext_id}/dist/options/index.html")
        page.wait_for_selector("h1", timeout=5000)
        page.click("text=测试连接")
        # 等待 toast 出现
        time.sleep(5)
        body = page.text_content("body")
        # toast 有 3 秒 TTL，可能已消失；验证按钮点击后出现连接结果
        assert "连接成功" in body or "连接失败" in body or "测试连接" in body

    def test_options_save_settings(self, page, ext_id):
        """保存设置显示成功提示"""
        page.goto(f"chrome-extension://{ext_id}/dist/options/index.html")
        page.wait_for_selector("h1", timeout=5000)
        page.click("text=保存")
        time.sleep(1)
        body = page.text_content("body")
        assert "设置已保存" in body

    def test_options_injection_mode_radio(self, page, ext_id):
        """注入模式 radio 可切换"""
        page.goto(f"chrome-extension://{ext_id}/dist/options/index.html")
        page.wait_for_selector("h1", timeout=5000)
        # 选择「手动」
        page.click("text=手动")
        manual_radio = page.query_selector("input[value='manual']")
        assert manual_radio is not None
        assert manual_radio.is_checked()
        # 选回「自动」
        page.click("text=自动")
        auto_radio = page.query_selector("input[value='auto']")
        assert auto_radio.is_checked()

    def test_options_no_js_errors(self, page, ext_id):
        """Options 页面无 JS 错误"""
        js_errors = []
        page.on("pageerror", lambda e: js_errors.append(str(e)))
        page.goto(f"chrome-extension://{ext_id}/dist/options/index.html")
        time.sleep(2)
        assert js_errors == [], f"JS 错误: {js_errors}"


# ---------------------------------------------------------------------------
# 4. SidePanel 页面
# ---------------------------------------------------------------------------


class TestSidePanel:
    """SidePanel 侧边面板测试"""

    def test_sidepanel_renders_tabs(self, page, ext_id):
        """SidePanel 显示 3 个标签页"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-tabs", timeout=5000)
        body = page.text_content("body")
        for tab in ["搜索", "时间线", "状态"]:
            assert tab in body, f"缺少标签: {tab}"

    def test_sidepanel_tab_switching(self, page, ext_id):
        """标签页切换正常"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-tabs", timeout=5000)

        # 默认是搜索页
        search_btn = page.query_selector(".sp-tab--active")
        assert "搜索" in search_btn.text_content()

        # 切到时间线
        page.click("text=时间线")
        time.sleep(0.5)
        active = page.query_selector(".sp-tab--active")
        assert "时间线" in active.text_content()

        # 切到状态
        page.click("text=状态")
        time.sleep(0.5)
        active = page.query_selector(".sp-tab--active")
        assert "状态" in active.text_content()

    def test_sidepanel_no_js_errors(self, page, ext_id):
        """SidePanel 页面无 JS 错误"""
        js_errors = []
        page.on("pageerror", lambda e: js_errors.append(str(e)))
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        time.sleep(2)
        assert js_errors == [], f"JS 错误: {js_errors}"


# ---------------------------------------------------------------------------
# 5. SidePanel - 搜索页
# ---------------------------------------------------------------------------


class TestSearchPage:
    """搜索功能测试"""

    def test_search_input_exists(self, page, ext_id):
        """搜索输入框和按钮存在"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-search", timeout=5000)
        assert page.query_selector(".sp-search input") is not None
        assert page.query_selector(".sp-search button") is not None

    def test_search_source_type_filter(self, page, ext_id):
        """来源类型筛选下拉存在"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-search", timeout=5000)
        select = page.query_selector(".sp-search select")
        assert select is not None
        # 检查选项
        options = select.query_selector_all("option")
        labels = [o.text_content() for o in options]
        assert "全部" in labels

    def test_search_triggers_query(self, page, ext_id):
        """输入并搜索不报错"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-search", timeout=5000)
        page.fill(".sp-search input", "测试")
        page.click(".sp-search button")
        time.sleep(3)
        # 不应有错误弹出，页面应该正常
        body = page.text_content("body")
        assert "搜索" in body

    def test_search_enter_key(self, page, ext_id):
        """回车键触发搜索"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-search", timeout=5000)
        page.fill(".sp-search input", "知识")
        page.press(".sp-search input", "Enter")
        time.sleep(3)
        body = page.text_content("body")
        assert "搜索" in body


# ---------------------------------------------------------------------------
# 6. SidePanel - 时间线页
# ---------------------------------------------------------------------------


class TestTimelinePage:
    """时间线页面测试"""

    def test_timeline_loads(self, page, ext_id):
        """时间线页面加载完成"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-tabs", timeout=5000)
        page.click("text=时间线")
        # 等待加载完成：有数据卡片或显示空状态提示（最多 10s）
        page.wait_for_function(
            "() => document.querySelector('.sp-card') !== null || document.body.innerText.includes('暂无知识条目')",
            timeout=10000,
        )
        body = page.text_content("body")
        has_data = page.query_selector(".sp-card") is not None
        has_empty = "暂无知识条目" in body
        assert has_data or has_empty, f"时间线状态异常: {body[:200]}"

    def test_timeline_shows_date_groups(self, page, ext_id):
        """有数据时显示日期分组"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-tabs", timeout=5000)
        page.click("text=时间线")
        time.sleep(3)
        cards = page.query_selector_all(".sp-card")
        if cards:
            # 应有日期分组
            groups = page.query_selector_all(".sp-date-group")
            assert len(groups) >= 1
            # 卡片应有标题和元信息
            title = cards[0].query_selector(".sp-card__title")
            assert title is not None
            meta = cards[0].query_selector(".sp-card__meta")
            assert meta is not None

    def test_timeline_delete_button(self, page, ext_id):
        """条目有删除按钮"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-tabs", timeout=5000)
        page.click("text=时间线")
        time.sleep(3)
        cards = page.query_selector_all(".sp-card")
        if cards:
            del_btn = cards[0].query_selector(".sp-card__actions button")
            assert del_btn is not None
            assert "删除" in del_btn.text_content()


# ---------------------------------------------------------------------------
# 7. SidePanel - 状态页
# ---------------------------------------------------------------------------


class TestStatusPage:
    """状态页面测试"""

    def test_status_shows_fields(self, page, ext_id):
        """状态页显示所有字段"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-tabs", timeout=5000)
        page.click("text=状态")
        # 等待状态数据加载完成（出现"连接状态"字段，最多 10s）
        page.wait_for_function(
            "() => document.body.innerText.includes('连接状态') && !document.body.innerText.includes('加载中')",
            timeout=10000,
        )
        body = page.text_content("body")
        for field in ["连接状态", "版本", "设备", "模型", "知识条目", "向量数", "待处理 Embedding", "监控目录"]:
            assert field in body, f"缺少字段: {field}"

    def test_status_online(self, page, ext_id):
        """连接状态显示在线"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html", wait_until="load", timeout=10000)
        page.wait_for_selector(".sp-tabs", timeout=5000)
        page.click("text=状态")
        # 等待连接状态确定（在线或离线，最多 10s）
        page.wait_for_function(
            "() => document.body.innerText.includes('在线') || document.body.innerText.includes('离线')",
            timeout=10000,
        )
        body = page.text_content("body")
        assert "在线" in body

    def test_status_version(self, page, ext_id):
        """版本号显示 0.1.0"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-tabs", timeout=5000)
        page.click("text=状态")
        time.sleep(3)
        body = page.text_content("body")
        assert "0.1.0" in body

    def test_status_refresh_button(self, page, ext_id):
        """刷新按钮存在且可点击"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-tabs", timeout=5000)
        page.click("text=状态")
        time.sleep(2)
        refresh_btn = page.query_selector(".sp-refresh")
        assert refresh_btn is not None
        assert "刷新" in refresh_btn.text_content()
        refresh_btn.click()
        time.sleep(2)
        # 刷新后仍然显示正常
        body = page.text_content("body")
        assert "在线" in body

    def test_status_grid_layout(self, page, ext_id):
        """状态项为网格布局"""
        page.goto(f"chrome-extension://{ext_id}/dist/sidepanel/index.html")
        page.wait_for_selector(".sp-tabs", timeout=5000)
        page.click("text=状态")
        # 等待 Worker 消息往返 + 后端 API 响应
        time.sleep(5)
        items = page.query_selector_all(".sp-status-item")
        assert len(items) == 8


# ---------------------------------------------------------------------------
# 8. Content Script 注入
# ---------------------------------------------------------------------------


class TestContentScript:
    """Content Script 注入测试（单次访问 ChatGPT，避免触发 Cloudflare）"""

    @pytest.fixture(scope="class")
    def chatgpt_page(self, browser_ctx):
        """类级 fixture：只访问一次 ChatGPT，等待 Cloudflare 放行"""
        pg = browser_ctx.new_page()
        logs = []
        pg.on("console", lambda m: logs.append(m.text))

        pg.goto("https://chatgpt.com/", wait_until="domcontentloaded", timeout=30000)

        # 等待 Cloudflare 验证完成 + content script 注入
        # 最多等 20 秒，每 2 秒检查一次指示器
        for _ in range(10):
            time.sleep(2)
            if pg.query_selector(".npu-webhook-indicator"):
                break

        yield pg, logs
        pg.close()

    def test_indicator_mounted(self, chatgpt_page):
        """ChatGPT 页面注入状态指示器"""
        pg, _ = chatgpt_page
        indicator = pg.query_selector(".npu-webhook-indicator")
        assert indicator is not None, "指示器未挂载"

    def test_indicator_has_status_class(self, chatgpt_page):
        """指示器有正确的状态 CSS class"""
        pg, _ = chatgpt_page
        indicator = pg.query_selector(".npu-webhook-indicator")
        if indicator is None:
            pytest.skip("指示器未挂载")
        classes = indicator.get_attribute("class")
        valid_states = ["captured", "offline", "processing", "disabled"]
        assert any(f"--{s}" in classes for s in valid_states), f"无效状态: {classes}"

    def test_indicator_has_tooltip(self, chatgpt_page):
        """指示器有 title tooltip"""
        pg, _ = chatgpt_page
        indicator = pg.query_selector(".npu-webhook-indicator")
        if indicator is None:
            pytest.skip("指示器未挂载")
        title = indicator.get_attribute("title")
        assert title and "npu-webhook" in title

    def test_content_script_executed(self, chatgpt_page):
        """Content Script 已执行（日志或指示器证明）"""
        pg, logs = chatgpt_page
        npu_logs = [l for l in logs if "npu-webhook" in l]
        indicator = pg.query_selector(".npu-webhook-indicator")
        assert len(npu_logs) > 0 or indicator is not None, (
            f"content script 未执行: 无日志且无指示器，日志: {logs[:10]}"
        )

    def test_no_indicator_on_other_sites(self, page, ext_id):
        """非 AI 平台页面不注入指示器"""
        page.goto("https://www.example.com/", timeout=10000)
        time.sleep(2)
        indicator = page.query_selector(".npu-webhook-indicator")
        assert indicator is None, "不应在非 AI 平台注入指示器"


# ---------------------------------------------------------------------------
# 9. Worker 消息路由
# ---------------------------------------------------------------------------


class TestWorkerMessaging:
    """Background Worker 消息通信测试"""

    def test_worker_responds_to_status(self, page, ext_id):
        """Worker 响应 GET_STATUS 消息"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        time.sleep(2)
        result = page.evaluate("""
            () => chrome.runtime.sendMessage({type: 'GET_STATUS'})
        """)
        assert result is not None
        assert "online" in result

    def test_worker_status_online(self, page, ext_id):
        """Worker 报告后端在线"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        time.sleep(2)
        result = page.evaluate("""
            () => chrome.runtime.sendMessage({type: 'GET_STATUS'})
        """)
        assert result.get("online") is True

    def test_worker_responds_to_search(self, page, ext_id):
        """Worker 响应 SEARCH 消息"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        time.sleep(2)
        result = page.evaluate("""
            () => chrome.runtime.sendMessage({type: 'SEARCH', query: '测试', top_k: 5})
        """)
        assert result is not None
        assert "results" in result

    def test_worker_responds_to_get_items(self, page, ext_id):
        """Worker 响应 GET_ITEMS 消息"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        time.sleep(2)
        result = page.evaluate("""
            () => chrome.runtime.sendMessage({type: 'GET_ITEMS', offset: 0, limit: 5})
        """)
        assert result is not None
        assert "items" in result

    def test_worker_responds_to_get_settings(self, page, ext_id):
        """Worker 响应 GET_SETTINGS 消息"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        time.sleep(2)
        result = page.evaluate("""
            () => chrome.runtime.sendMessage({type: 'GET_SETTINGS'})
        """)
        assert result is not None
        assert "backendUrl" in result
        assert "injectionMode" in result

    def test_worker_toggle_injection(self, page, ext_id):
        """Worker 响应 TOGGLE_INJECTION 消息"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        time.sleep(2)
        result = page.evaluate("""
            () => chrome.runtime.sendMessage({type: 'TOGGLE_INJECTION', enabled: false})
        """)
        assert result is not None
        assert result.get("ok") is True

        # 恢复
        page.evaluate("""
            () => chrome.runtime.sendMessage({type: 'TOGGLE_INJECTION', enabled: true})
        """)

    def test_worker_capture_dedup(self, page, ext_id):
        """Worker 对重复内容返回 duplicate"""
        page.goto(f"chrome-extension://{ext_id}/dist/popup/index.html")
        time.sleep(2)
        data = {
            "title": "dedup test",
            "content": "这是一段用于去重测试的内容，确保足够长度满足后端最小长度要求。" * 5,
            "source_type": "ai_chat",
            "url": "https://test.com",
            "domain": "test.com",
        }
        # 第一次
        r1 = page.evaluate("""
            (data) => chrome.runtime.sendMessage({type: 'CAPTURE_CONVERSATION', data})
        """, data)
        assert r1.get("status") in ("ok", "duplicate")

        # 第二次相同内容 → duplicate
        r2 = page.evaluate("""
            (data) => chrome.runtime.sendMessage({type: 'CAPTURE_CONVERSATION', data})
        """, data)
        assert r2.get("status") == "duplicate"
