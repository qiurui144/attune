/**
 * 后端 API 封装
 */

const DEFAULT_BASE_URL = 'http://localhost:18900/api/v1';

export class API {
  constructor(baseUrl = DEFAULT_BASE_URL) {
    this.baseUrl = baseUrl;
  }

  /** 从 chrome.storage 重新加载 baseUrl */
  async reloadBaseUrl() {
    try {
      const result = await chrome.storage.local.get('settings');
      const settings = result.settings;
      if (settings?.backendUrl) {
        this.baseUrl = settings.backendUrl.replace(/\/+$/, '') + '/api/v1';
      }
    } catch { /* content script 环境可能无权限 */ }
  }

  async request(path, options = {}) {
    const resp = await fetch(`${this.baseUrl}${path}`, {
      headers: { 'Content-Type': 'application/json', ...options.headers },
      ...options,
    });
    if (!resp.ok) throw new Error(`API error: ${resp.status}`);
    return resp.json();
  }

  // --- 基础 ---
  health() { return this.request('/status/health'); }
  status() { return this.request('/status'); }

  // --- 知识注入 ---
  ingest(data) { return this.request('/ingest', { method: 'POST', body: JSON.stringify(data) }); }

  // --- 搜索 ---
  search(query, { topK = 10, sourceTypes } = {}) {
    let url = `/search?q=${encodeURIComponent(query)}&top_k=${topK}`;
    if (sourceTypes) url += `&source_types=${encodeURIComponent(sourceTypes)}`;
    return this.request(url);
  }

  searchRelevant(data) {
    return this.request('/search/relevant', { method: 'POST', body: JSON.stringify(data) });
  }

  // --- 条目 CRUD ---
  items({ offset = 0, limit = 20, sourceType } = {}) {
    let url = `/items?offset=${offset}&limit=${limit}`;
    if (sourceType) url += `&source_type=${encodeURIComponent(sourceType)}`;
    return this.request(url);
  }

  getItem(id) { return this.request(`/items/${id}`); }

  deleteItem(id) { return this.request(`/items/${id}`, { method: 'DELETE' }); }

  updateItem(id, data) {
    return this.request(`/items/${id}`, { method: 'PATCH', body: JSON.stringify(data) });
  }

  // --- 设置 ---
  getSettings() { return this.request('/settings'); }

  updateSettings(data) {
    return this.request('/settings', { method: 'PATCH', body: JSON.stringify(data) });
  }

  // --- 索引 ---
  indexStatus() { return this.request('/index/status'); }

  // --- G1 浏览信号 (W3 batch B, 2026-04-27) ---
  // per spec docs/superpowers/specs/2026-04-27-w3-batch-b-design.md §3
  recordBrowseSignals(signals) {
    return this.request('/browse_signals', {
      method: 'POST',
      body: JSON.stringify({ signals }),
    });
  }
  listBrowseSignals(limit = 20) {
    return this.request(`/browse_signals?limit=${limit}`);
  }
  clearBrowseSignals(domain) {
    const q = domain ? `?domain=${encodeURIComponent(domain)}` : '';
    return this.request(`/browse_signals${q}`, { method: 'DELETE' });
  }
}

export const api = new API();
