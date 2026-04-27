// G1 通用浏览状态捕获 content script (W3 batch B, 2026-04-27)
//
// per spec docs/superpowers/specs/2026-04-27-w3-batch-b-design.md §4
//
// 隐私强约束（per reviewer S1 + S2 修复）：
// 1. 默认 opt-out — 仅 chrome.storage.local["browseWhitelist"] 含当前 hostname 才捕获
// 2. HARD_BLACKLIST 分两层 — hostname 正则 + pathname 正则（避免 /login/ 误判 /login-tips）
// 3. incognito 永不捕获（chrome.extension.inIncognitoContext 显式检查）
// 4. 不抓 password / form 字段；仅捕获 URL + title + dwell + scroll + copy + visit

(function () {
  'use strict';

  // per reviewer S1：incognito 模式硬阻断（manifest 默认 spanning 模式扩展会在隐私窗口加载）
  if (typeof chrome !== 'undefined' && chrome.extension && chrome.extension.inIncognitoContext) {
    return;
  }

  // per reviewer S2：分两层判断 — host 正则 + path 正则，避免单一正则误报
  // hostname 层：匹配整个域名形态（银行/政府/密码管理器）
  const HARD_BLACKLIST_HOST = [
    /\.bank\b/i,
    /\.medical\b/i,
    /\.gov(?:$|:)/i,
    /1password\.com$/i,
    /lastpass\.com$/i,
    /bitwarden\.com$/i,
    /accounts\.google\.com$/i,
  ];
  // pathname 层：匹配登录 / 密码相关 path（如 github.com/login，paypal.com/signin）
  const HARD_BLACKLIST_PATH = [
    /\/login(?:\/|$)/i,
    /\/signin(?:\/|$)/i,
    /\/sign-in(?:\/|$)/i,
    /\/log-in(?:\/|$)/i,
    /\/password(?:\/|$)/i,
    /\/auth(?:\/|$)/i,
    /\/oauth(?:\/|$)/i,
  ];

  const hostname = location.hostname;
  if (!hostname || HARD_BLACKLIST_HOST.some((re) => re.test(hostname))) {
    return;
  }
  if (HARD_BLACKLIST_PATH.some((re) => re.test(location.pathname))) {
    return;
  }
  // 不在 chrome:// / about: / 扩展自身
  if (location.protocol === 'chrome:' || location.protocol === 'about:' ||
      location.protocol === 'chrome-extension:') {
    return;
  }

  let dwellStart = Date.now();
  let dwellMs = 0;
  let scrollPctMax = 0;
  let copyCount = 0;
  let flushed = false;

  // dwell timer：visibilitychange + page lifecycle
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
      dwellMs += Date.now() - dwellStart;
      tryFlush('visibility-hidden');
    } else {
      dwellStart = Date.now();
    }
  });

  // scroll 深度跟踪（IntersectionObserver 看 body 底部）
  // 用 documentElement.scrollHeight 算百分比，避免依赖第三方 sentinel
  function updateScrollPct() {
    const total = document.documentElement.scrollHeight - window.innerHeight;
    if (total <= 0) return;
    const pct = Math.round(((window.scrollY + window.innerHeight) / document.documentElement.scrollHeight) * 100);
    if (pct > scrollPctMax) scrollPctMax = Math.min(100, pct);
  }
  window.addEventListener('scroll', updateScrollPct, { passive: true });

  // copy 事件
  document.addEventListener('copy', () => {
    copyCount++;
  });

  // 页面卸载前 flush
  window.addEventListener('beforeunload', () => {
    if (!document.hidden) {
      dwellMs += Date.now() - dwellStart;
    }
    tryFlush('beforeunload');
  });

  // pagehide (BFCache 友好)
  window.addEventListener('pagehide', () => tryFlush('pagehide'));

  function tryFlush(reason) {
    if (flushed) return; // 防重复
    if (dwellMs < 1000) return; // 太短不上报（< 1 秒）

    chrome.storage.local.get(['browseWhitelist', 'browsePaused'], (cfg) => {
      if (cfg.browsePaused) return; // 全局 Pause
      const whitelist = cfg.browseWhitelist || [];
      if (whitelist.length === 0) return; // 默认 opt-out — 用户未加任何域名
      const matched = whitelist.some((pattern) => {
        if (!pattern) return false;
        return hostname === pattern || hostname.endsWith('.' + pattern);
      });
      if (!matched) return;

      flushed = true;
      const payload = {
        url: location.href,
        title: document.title || '',
        dwell_ms: dwellMs,
        scroll_pct: scrollPctMax,
        copy_count: copyCount,
        visit_count: 1,
      };
      try {
        chrome.runtime.sendMessage({ type: 'BROWSE_SIGNAL', payload, reason });
      } catch (_) {
        // service worker 可能已休眠 — background 会在 onMessage 唤醒时重建队列
      }
    });
  }
})();
