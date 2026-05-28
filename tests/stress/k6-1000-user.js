// k6 stress test — 1000 user / 4 path round-robin (v1.0.5 framework, 未真跑)
//
// 用途:
//   - cloud + attune-server 联合压测,验证 P50/P95/P99 SLO(per docs/PERFORMANCE-BASELINE-v1-0-5.md)
//   - 不在 sandbox 内真跑,等 user 在 cloud 真服务器(或 K3 一体机)启服后再 dispatch
//
// 运行(等 user 准备真服务器后):
//   CLOUD_URL=https://gateway.engi-stack.com \
//   ATTUNE_SERVER_URL=https://attune-server.local:18900 \
//   AUTH_TOKEN=<bearer> \
//   k6 run tests/stress/k6-1000-user.js --out json=reports/stress/k6-1000-$(date +%s).json
//
// 4 路径覆盖:
//   1. signup-light  — accounts /api/v1/auth/check (read-only,heartbeat 兜底)
//   2. login         — accounts /api/v1/auth/refresh (token refresh,模拟活跃 session)
//   3. chat          — llm-gateway /v1/chat/completions (核心成本路径,token 计费)
//   4. search        — attune-server /api/v1/search (本地 vault 检索,无 LLM 调用)
//
// SLO(per § 任务 1):
//   - P50 chat < 2000 ms
//   - P95 chat < 5000 ms
//   - P99 chat < 10000 ms
//   - P50 search < 100 ms
//   - P99 search < 1000 ms
//   - http_req_failed rate < 0.01 (< 1% error)

import http from 'k6/http';
import { check, sleep } from 'k6';
import { Rate, Trend } from 'k6/metrics';

// ─────────────────────────────────────────────────────────────────────
// 自定义 metric — 按路径分桶,便于 grafana / k6 cloud 区分
// ─────────────────────────────────────────────────────────────────────

const chatFailRate = new Rate('chat_fail_rate');
const searchFailRate = new Rate('search_fail_rate');
const chatLatency = new Trend('chat_latency_ms', true);
const searchLatency = new Trend('search_latency_ms', true);

// ─────────────────────────────────────────────────────────────────────
// stages — 4 phase load profile(模拟真实生产 ramp)
// ─────────────────────────────────────────────────────────────────────

export const options = {
  stages: [
    { duration: '2m', target: 100 },   // warm up
    { duration: '5m', target: 500 },   // sustained mid
    { duration: '5m', target: 1000 },  // peak 1000 concurrent
    { duration: '2m', target: 0 },     // graceful ramp down
  ],
  thresholds: {
    // 全局 SLO
    http_req_duration: ['p(99)<5000'],          // p99 全路径 < 5s
    http_req_failed:   ['rate<0.01'],            // < 1% 失败
    // 按路径 SLO
    'chat_latency_ms{path:chat}':    ['p(50)<2000', 'p(95)<5000', 'p(99)<10000'],
    'search_latency_ms{path:search}': ['p(50)<100',  'p(99)<1000'],
    chat_fail_rate:    ['rate<0.02'],
    search_fail_rate:  ['rate<0.01'],
  },
  // 默认 user-agent 标识为 attune-stress(便于 server log 区分)
  userAgent: 'attune-stress/v1.0.5 k6',
};

// ─────────────────────────────────────────────────────────────────────
// 环境变量 — 启动前 export(per CLAUDE.md § Secrets 严禁硬编码)
// ─────────────────────────────────────────────────────────────────────

const CLOUD_URL = __ENV.CLOUD_URL || 'https://gateway.engi-stack.com';
const ATTUNE_SERVER_URL = __ENV.ATTUNE_SERVER_URL || 'http://localhost:18900';
const AUTH_TOKEN = __ENV.AUTH_TOKEN || '';

if (!AUTH_TOKEN) {
  // 真跑前必须 export AUTH_TOKEN(走 attune login 拿,不入 git)
  // k6 不抛 panic,但运行时打印警告
  console.warn('[stress] AUTH_TOKEN empty — login/chat path will 401');
}

const headers = {
  'Content-Type': 'application/json',
  Authorization: `Bearer ${AUTH_TOKEN}`,
};

// ─────────────────────────────────────────────────────────────────────
// 4 路径 — round-robin per VU iter
// ─────────────────────────────────────────────────────────────────────

export default function () {
  const iter = __ITER % 4;

  if (iter === 0) {
    // (1) signup-light — 检查 user 状态(read-only,模拟 heartbeat)
    const r = http.get(`${CLOUD_URL}/accounts/api/v1/auth/check`, { headers, tags: { path: 'signup' } });
    check(r, { 'signup-light 200': (resp) => resp.status === 200 });
  } else if (iter === 1) {
    // (2) login — token refresh(模拟活跃 session)
    const r = http.post(
      `${CLOUD_URL}/accounts/api/v1/auth/refresh`,
      JSON.stringify({}),
      { headers, tags: { path: 'login' } },
    );
    check(r, { 'login 200': (resp) => resp.status === 200 });
  } else if (iter === 2) {
    // (3) chat — 核心成本路径(LLM token 计费),attune-pro membership gateway
    const body = JSON.stringify({
      model: 'deepseek-v4-flash',
      messages: [
        { role: 'system', content: 'You are a knowledge assistant.' },
        { role: 'user', content: 'What is the meaning of life in one sentence?' },
      ],
      max_tokens: 64,
    });
    const start = Date.now();
    const r = http.post(`${CLOUD_URL}/llm-gateway/v1/chat/completions`, body, {
      headers,
      tags: { path: 'chat' },
      timeout: '30s',
    });
    chatLatency.add(Date.now() - start, { path: 'chat' });
    const ok = check(r, {
      'chat 200': (resp) => resp.status === 200,
      'chat has choices': (resp) => {
        try {
          const j = resp.json();
          return j && j.choices && j.choices.length > 0;
        } catch (_) {
          return false;
        }
      },
    });
    chatFailRate.add(!ok);
  } else {
    // (4) search — 本地 vault FTS + vector 检索(零 LLM 成本)
    const start = Date.now();
    const r = http.get(`${ATTUNE_SERVER_URL}/api/v1/search?q=knowledge+graph&limit=20`, {
      headers,
      tags: { path: 'search' },
      timeout: '5s',
    });
    searchLatency.add(Date.now() - start, { path: 'search' });
    const ok = check(r, { 'search 200': (resp) => resp.status === 200 });
    searchFailRate.add(!ok);
  }

  // sleep 0.5-1.5s — 模拟真实 user think-time(不要 zero-sleep 灌满)
  sleep(0.5 + Math.random());
}

// ─────────────────────────────────────────────────────────────────────
// teardown — 跑完打印汇总(k6 已经自带 stdout summary,这里只补 release-note 友好格式)
// ─────────────────────────────────────────────────────────────────────

export function handleSummary(data) {
  return {
    'stdout': textSummary(data),
    'reports/stress/k6-summary.json': JSON.stringify(data, null, 2),
  };
}

function textSummary(data) {
  const m = data.metrics;
  const fmt = (v) => (v === undefined ? 'n/a' : v.toFixed(2));
  return `
=== attune v1.0.5 stress test summary ===
duration:      ${fmt(data.state.testRunDurationMs / 1000)} s
total_reqs:    ${m.http_reqs ? m.http_reqs.values.count : 'n/a'}
error_rate:    ${fmt((m.http_req_failed?.values?.rate || 0) * 100)} %

chat p50:      ${fmt(m['chat_latency_ms']?.values?.['p(50)'])} ms (SLO < 2000)
chat p95:      ${fmt(m['chat_latency_ms']?.values?.['p(95)'])} ms (SLO < 5000)
chat p99:      ${fmt(m['chat_latency_ms']?.values?.['p(99)'])} ms (SLO < 10000)

search p50:    ${fmt(m['search_latency_ms']?.values?.['p(50)'])} ms (SLO < 100)
search p99:    ${fmt(m['search_latency_ms']?.values?.['p(99)'])} ms (SLO < 1000)
`;
}
