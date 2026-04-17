/**
 * Background Service Worker — 消息路由 + API 调度 + 去重 + 右键菜单 + 摘要
 */

import { API } from '../shared/api.js';
import { getSettings } from '../shared/storage.js';
import { MSG, sendToTab } from '../shared/messages.js';

const api = new API();

// --- 去重缓存 (hash -> timestamp) ---
const dedup = new Map();
const DEDUP_TTL = 60 * 60 * 1000; // 1h

// --- 预取缓存 (queryHash -> {results, ts}) ---
const prefetchCache = new Map();
const PREFETCH_TTL = 30 * 1000; // 30s，覆盖"打字→发送"时间窗口

// --- 连接状态 ---
let backendOnline = false;
let injectionEnabled = true;

// --- 初始化 ---
chrome.runtime.onInstalled.addListener(() => {
  console.log('[npu-webhook] Extension installed');
  // 创建右键菜单
  chrome.contextMenus.create({
    id: 'npu-save-selection',
    title: '保存到知识库',
    contexts: ['selection'],
  });
  initState();
});

chrome.runtime.onStartup?.addListener(() => {
  initState();
});

async function initState() {
  await api.reloadBaseUrl();
  try {
    const session = await chrome.storage.session.get('dedup');
    if (session.dedup) {
      for (const [k, v] of Object.entries(session.dedup)) {
        dedup.set(k, v);
      }
    }
  } catch { /* */ }
  try {
    const result = await chrome.storage.local.get('injectionEnabled');
    if (result.injectionEnabled !== undefined) injectionEnabled = result.injectionEnabled;
  } catch { /* */ }
  healthCheck();
}

// --- 右键菜单：选中文本入库 ---
chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === 'npu-save-selection' && info.selectionText) {
    const data = {
      title: info.selectionText.slice(0, 100),
      content: info.selectionText,
      source_type: 'selection',
      url: tab?.url || '',
      domain: tab?.url ? new URL(tab.url).hostname : '',
      metadata: { source: 'context_menu' },
    };

    try {
      await handleCapture(data);
      console.log('[npu-webhook] Selection saved:', data.title);
    } catch (err) {
      console.error('[npu-webhook] Save selection failed:', err);
    }
  }
});

// --- 消息路由 ---
chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  handleMessage(msg, sender).then(sendResponse).catch((err) => {
    console.error('[npu-webhook] Message handler error:', err);
    sendResponse({ error: err.message });
  });
  return true;
});

async function handleMessage(msg, sender) {
  switch (msg.type) {
    case MSG.CAPTURE_CONVERSATION:
      return handleCapture(msg.data);

    case MSG.SUMMARIZE_AND_SAVE:
      return handleSummarizeAndSave(msg.data);

    case MSG.PREFETCH: {
      // [deprecated] 2026-04-12: injector 已弃用，PREFETCH 仅保留供 API 兼容
      // 打字时后台预取，结果存入缓存供注入使用（不阻塞用户）
      const prefetchKey = djb2((msg.query || '') + JSON.stringify(msg.source_types || null));
      if (!prefetchCache.has(prefetchKey) && backendOnline) {
        api.searchRelevant({
          query: msg.query,
          top_k: msg.top_k || 3,
          context: msg.context || null,
          min_score: msg.min_score || 0.3,
          source_types: msg.source_types || null,
        }).then((results) => {
          prefetchCache.set(prefetchKey, { results, ts: Date.now() });
        }).catch(() => {}); // 静默失败，注入时会回退到实时请求
      }
      return { ok: true };
    }

    case MSG.SEARCH_RELEVANT: {
      // [deprecated] 2026-04-12: injector 已弃用，SEARCH_RELEVANT 保留供 sidepanel/API 使用
      // 先查预取缓存，命中则直接返回（<1ms）
      const cacheKey = djb2((msg.query || '') + JSON.stringify(msg.source_types || null));
      const cached = prefetchCache.get(cacheKey);
      if (cached && Date.now() - cached.ts < PREFETCH_TTL) {
        prefetchCache.delete(cacheKey); // 消费后删除
        return cached.results;
      }
      return api.searchRelevant({
        query: msg.query,
        top_k: msg.top_k || 3,
        context: msg.context || null,
        min_score: msg.min_score || 0,
        source_types: msg.source_types || null,
      });
    }

    case MSG.SAVE_SELECTION:
      return handleCapture(msg.data);

    case MSG.GET_STATUS: {
      let status = { online: backendOnline, injection_enabled: injectionEnabled };
      if (backendOnline) {
        try {
          const s = await api.status();
          status = { ...status, ...s };
        } catch { /* */ }
      }
      return status;
    }

    case MSG.SEARCH:
      return api.search(msg.query, { topK: msg.top_k, sourceTypes: msg.source_types });

    case MSG.GET_ITEMS:
      return api.items({ offset: msg.offset, limit: msg.limit, sourceType: msg.source_type });

    case MSG.GET_SETTINGS:
      return getSettings();

    case MSG.TOGGLE_INJECTION: {
      injectionEnabled = msg.enabled;
      await chrome.storage.local.set({ injectionEnabled });
      const tabs = await chrome.tabs.query({});
      for (const tab of tabs) {
        try {
          await sendToTab(tab.id, MSG.TOGGLE_INJECTION, { enabled: injectionEnabled });
        } catch { /* */ }
      }
      return { ok: true };
    }

    case MSG.SETTINGS_UPDATED: {
      // 立即重载 baseUrl，无需等下一次健康检查
      await api.reloadBaseUrl();
      return { ok: true };
    }

    case MSG.OPEN_SIDEPANEL:
      if (sender.tab) {
        await chrome.sidePanel.open({ tabId: sender.tab.id });
      }
      return { ok: true };

    default:
      return { error: `Unknown message type: ${msg.type}` };
  }
}

// --- 对话捕获去重 + 入库 ---
function djb2(str) {
  let h = 5381;
  for (let i = 0; i < str.length; i++) {
    h = ((h << 5) + h + str.charCodeAt(i)) >>> 0;
  }
  return h.toString(36);
}

async function handleCapture(data) {
  if (!data?.content) return { error: 'No content' };

  const hash = djb2(data.content);
  if (dedup.has(hash)) return { status: 'duplicate' };

  dedup.set(hash, Date.now());
  persistDedup();

  try {
    const result = await api.ingest(data);
    return { status: 'ok', id: result.id };
  } catch (err) {
    dedup.delete(hash);
    throw err;
  }
}

// --- 对话摘要后入库 ---
async function handleSummarizeAndSave(data) {
  if (!data?.content) return { error: 'No content' };

  // 先占坑：避免 await ollamaSummarize 期间并发请求绕过去重
  const hash = djb2(data.content);
  if (dedup.has(hash)) return { status: 'duplicate' };
  dedup.set(hash, Date.now());
  persistDedup();

  // 用 Ollama 生成摘要（失败时回退到全文，不影响入库）
  let summary = null;
  try {
    summary = await ollamaSummarize(data.content);
  } catch (err) {
    console.warn('[npu-webhook] Summarize failed, saving full text:', err);
  }

  // 保存：摘要成功时附带摘要，失败时仅保存全文，更新 has_summary 标记
  const saveData = {
    ...data,
    content: summary
      ? `[摘要]\n${summary}\n\n[原文]\n${data.content}`
      : data.content,
    metadata: {
      ...(data.metadata || {}),
      has_summary: summary ? 'true' : 'false', // 准确反映实际状态
    },
  };

  try {
    const result = await api.ingest(saveData);
    return { status: 'ok', id: result.id, summarized: !!summary };
  } catch (err) {
    dedup.delete(hash);
    throw err;
  }
}

async function ollamaSummarize(text) {
  const truncated = text.slice(0, 4000);
  const prompt = `请用中文简洁地总结以下 AI 对话的要点（问题、解决方案、关键结论），不超过 200 字：\n\n${truncated}`;

  const resp = await fetch('http://localhost:11434/api/generate', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      model: 'qwen2.5:1.5b',
      prompt,
      stream: false,
      options: { temperature: 0.3, num_predict: 300 },
    }),
  });

  if (!resp.ok) throw new Error(`Ollama generate: ${resp.status}`);
  const result = await resp.json();
  return result.response?.trim() || null;
}

function persistDedup() {
  const obj = Object.fromEntries(dedup);
  chrome.storage.session.set({ dedup: obj }).catch(() => {});
}

// --- 定期健康检查 + 缓存清理 ---
async function healthCheck() {
  try {
    await api.reloadBaseUrl();
    await api.health();
    backendOnline = true;
  } catch {
    backendOnline = false;
  }

  const now = Date.now();
  for (const [k, ts] of dedup) {
    if (now - ts > DEDUP_TTL) dedup.delete(k);
  }
  if (dedup.size > 0) persistDedup();

  // 清理过期预取缓存
  for (const [k, v] of prefetchCache) {
    if (now - v.ts > PREFETCH_TTL) prefetchCache.delete(k);
  }

  setTimeout(healthCheck, 30000);
}

initState();
