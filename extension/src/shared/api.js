/**
 * 后端 API 封装
 */

const DEFAULT_BASE_URL = 'http://localhost:18900/api/v1';

export class API {
  constructor(baseUrl = DEFAULT_BASE_URL) {
    this.baseUrl = baseUrl;
  }

  async request(path, options = {}) {
    const resp = await fetch(`${this.baseUrl}${path}`, {
      headers: { 'Content-Type': 'application/json', ...options.headers },
      ...options,
    });
    if (!resp.ok) throw new Error(`API error: ${resp.status}`);
    return resp.json();
  }

  health() { return this.request('/status/health'); }
  ingest(data) { return this.request('/ingest', { method: 'POST', body: JSON.stringify(data) }); }
  search(query) { return this.request(`/search?q=${encodeURIComponent(query)}`); }
  searchRelevant(data) { return this.request('/search/relevant', { method: 'POST', body: JSON.stringify(data) }); }
}

export const api = new API();
