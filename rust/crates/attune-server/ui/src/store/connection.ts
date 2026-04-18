/**
 * Attune 连接状态机 · 每 5 秒 ping 一次 /health，驱动 sidebar 状态点
 * 见 spec §7 "第一层 · 连接状态机"
 *
 * 状态流：
 *   online ──(ping 失败)──→ reconnecting ──(5 次失败 / >30s)──→ offline
 *      ↑                          │
 *      └──(ping 成功)─────────────┘
 */

import { apiCall, RETRY_POLICIES } from './api';
import { connectionState } from './signals';

const HEALTH_ENDPOINT = '/status/health';
const HEARTBEAT_INTERVAL = 5_000;
const INITIAL_BACKOFF = 500;
const MAX_BACKOFF = 30_000;
const MAX_CONSECUTIVE_FAILURES = 5;

type HealthResponse = {
  status: 'starting' | 'ok' | 'degraded' | 'down';
  vault_state?: string;
  db_ok?: boolean;
  ollama?: string;
};

let timer: ReturnType<typeof setTimeout> | null = null;
let consecutiveFailures = 0;
let currentBackoff = INITIAL_BACKOFF;

/** 启动连接监控（应用启动时调用一次） */
export function startConnectionMonitor(): void {
  stopConnectionMonitor();
  tick();
}

export function stopConnectionMonitor(): void {
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
  }
}

/** 用户手动触发重连（offline 状态下的 retry 按钮） */
export async function retryConnection(): Promise<void> {
  consecutiveFailures = 0;
  currentBackoff = INITIAL_BACKOFF;
  connectionState.value = 'reconnecting';
  await tick();
}

async function tick(): Promise<void> {
  try {
    await apiCall<HealthResponse>(HEALTH_ENDPOINT, {
      method: 'GET',
      retry: RETRY_POLICIES.heartbeat,
    });
    onSuccess();
  } catch {
    onFailure();
  }
}

function onSuccess(): void {
  consecutiveFailures = 0;
  currentBackoff = INITIAL_BACKOFF;
  connectionState.value = 'online';
  schedule(HEARTBEAT_INTERVAL);
}

function onFailure(): void {
  consecutiveFailures++;
  if (consecutiveFailures >= MAX_CONSECUTIVE_FAILURES) {
    connectionState.value = 'offline';
    // 停止自动 ping，等用户手动 retry
    return;
  }
  connectionState.value = 'reconnecting';
  schedule(currentBackoff);
  currentBackoff = Math.min(currentBackoff * 2, MAX_BACKOFF);
}

function schedule(delayMs: number): void {
  if (timer !== null) clearTimeout(timer);
  timer = setTimeout(() => {
    void tick();
  }, delayMs);
}
