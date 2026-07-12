/** useChat · Chat 核心逻辑封装
 * 见 spec §6 "API 契约" + §5 "business signals"
 *
 * 职责：
 *   - 加载 / 刷新 session 列表
 *   - 加载某 session 的消息历史
 *   - 发送消息（optimistic + 后台 API + 自动回填 session_id）
 *   - 新建 / 删除 session
 */

import { api, apiCall, RETRY_POLICIES } from '../store/api';
import {
  chatSessions,
  activeSessionId,
  messages,
  lastCostEstimate,
  lastFailedSend,
} from '../store/signals';
import type {
  ChatSession,
  Message,
  CostEstimate,
  AcpFlow,
  LocalSchedulerInfo,
} from '../store/signals';
import { t } from '../i18n';

// chat 请求超时上限。超过此值视为超时 → 给用户超时提示 + 重试入口，
// 而非无限转圈让用户怀疑卡死（90s 容忍较慢的本地大模型 / 云端长回答）。
const CHAT_TIMEOUT_MS = 90_000;
const LOCAL_SCHEDULER_JOB_POLL_INTERVAL_MS = 1_500;
const LOCAL_SCHEDULER_JOB_MAX_POLLS = 240;

// ── 后端响应类型 ─────────────────────────────────────────────
type SessionsResponse = {
  sessions: ChatSession[];
  total?: number;
};

type SessionDetailResponse = {
  session: ChatSession;
  messages: Array<{
    id?: string;
    role: 'user' | 'assistant' | 'system';
    content: string;
    citations?: Array<{ item_id: string; title: string; relevance: number }>;
    created_at?: string;
  }>;
};

type ChatResponse = {
  content: string;
  citations?: Array<{ item_id: string; title: string; relevance: number }>;
  knowledge_count?: number;
  session_id?: string;
  web_search_used?: boolean;
  hint?: string;
  cost_estimate?: CostEstimate;
  acp_flow?: AcpFlow;
  local_scheduler?: LocalSchedulerInfo;
};

type SchedulerJobStatus = LocalSchedulerInfo & {
  outputs?: unknown;
  cancel_requested?: boolean | null;
};

type LocalSchedulerJobResponse = {
  job: SchedulerJobStatus;
};

async function getSchedulerJob(jobId: string): Promise<LocalSchedulerJobResponse> {
  const encoded = encodeURIComponent(jobId);
  try {
    return await api.get<LocalSchedulerJobResponse>(`/chat/edge-scheduler/jobs/${encoded}`);
  } catch (e) {
    if (String(e).includes('404')) {
      return await api.get<LocalSchedulerJobResponse>(`/chat/local-scheduler/jobs/${encoded}`);
    }
    throw e;
  }
}

// 刚发送完一条消息后，sendMessage 会回填新 session_id，触发 ChatView 的
// activeSessionId effect 去 loadSession —— 但服务端 history 不持久化 acp_flow
// 流转 trace（live-only），重载会冲掉刚附在内存消息上的 acp_flow 块。
// 用此 guard 让那一次紧随发送的重载跳过，保留内存消息（含 acp_flow）。
let skipNextSessionLoad = false;
export function consumeSkipNextSessionLoad(): boolean {
  if (skipNextSessionLoad) {
    skipNextSessionLoad = false;
    return true;
  }
  return false;
}

// ── Session 管理 ─────────────────────────────────────────────
export async function loadSessions(): Promise<void> {
  try {
    const res = await api.get<SessionsResponse>('/chat/sessions');
    chatSessions.value = res.sessions ?? [];
  } catch {
    // 静默失败：sidebar 仍可工作，显示"还没有对话"
  }
}

export async function loadSession(sessionId: string): Promise<void> {
  try {
    const res = await api.get<SessionDetailResponse>(
      `/chat/sessions/${sessionId}`,
    );
    messages.value = (res.messages ?? []).map((m, i) => ({
      id: m.id ?? `msg-${i}`,
      role: m.role,
      content: m.content,
      citations: m.citations,
      created_at: m.created_at ?? new Date().toISOString(),
    }));
    activeSessionId.value = sessionId;
  } catch {
    messages.value = [];
  }
}

export function clearActiveSession(): void {
  activeSessionId.value = null;
  messages.value = [];
}

// ── 发送消息 ─────────────────────────────────────────────────
export type SendOptions = {
  onChunk?: (partial: string) => void;
  onDone?: (message: Message) => void;
};

// Important 2.6 修复：并发发送守卫
let sendInFlight = false;

export async function sendMessage(
  text: string,
  _opts?: SendOptions,
): Promise<void> {
  const trimmed = text.trim();
  if (!trimmed) return;
  if (sendInFlight) return; // 有请求在飞，忽略新发送
  sendInFlight = true;
  lastFailedSend.value = null; // 新发送 → 清掉上次失败态（重试入口消失）

  // Optimistic 用户消息
  const userMsg: Message = {
    id: `u-${Date.now()}`,
    role: 'user',
    content: trimmed,
    created_at: new Date().toISOString(),
  };
  messages.value = [...messages.value, userMsg];

  // 构造 history（不含刚加的这条 user，因为 backend 会自己把 message 作为最新 user）
  const history = messages.value
    .slice(0, -1)
    .filter((m) => m.role === 'user' || m.role === 'assistant')
    .map((m) => ({ role: m.role, content: m.content }));

  const currentSession = activeSessionId.value;

  // 超时标记 hoist 到函数作用域：apiCall 重试后会把 AbortError 包成 NetworkError，
  // 错误对象上的 name 丢失，故用此 flag 在 catch 里可靠区分超时 vs 一般失败。
  let timedOut = false;

  try {
    const body: Record<string, unknown> = {
      message: trimmed,
      history,
    };
    if (currentSession) body.session_id = currentSession;

    // 超时保护：AbortController + 定时器。超过 CHAT_TIMEOUT_MS 主动 abort，
    // fetch 抛 AbortError → 落入 catch 当作超时，给超时提示 + 重试入口。
    const ctrl = new AbortController();
    const timer = setTimeout(() => {
      timedOut = true;
      ctrl.abort();
    }, CHAT_TIMEOUT_MS);
    let res: ChatResponse;
    try {
      res = await apiCall<ChatResponse>('/chat', {
        method: 'POST',
        body: JSON.stringify(body),
        retry: RETRY_POLICIES.nonIdempotentWrite,
        signal: ctrl.signal,
      });
    } finally {
      clearTimeout(timer);
    }

    const assistantMsg: Message = {
      id: `a-${Date.now()}`,
      role: 'assistant',
      content: res.content,
      citations: res.citations,
      acp_flow: res.acp_flow,
      local_scheduler: res.local_scheduler,
      created_at: new Date().toISOString(),
    };
    messages.value = [...messages.value, assistantMsg];

    const localSchedulerJobId = res.local_scheduler?.job_id;
    if (localSchedulerJobId && !isLocalSchedulerTerminalStatus(res.local_scheduler?.status)) {
      void pollLocalSchedulerJob(assistantMsg.id, localSchedulerJobId);
    }

    // 若是第一次发送（无 session_id），回填 + 刷新列表。回填 activeSessionId 会触发
    // ChatView 的 loadSession —— 置 skip 让那次重载跳过，保留刚附 acp_flow 的内存消息。
    if (!currentSession && res.session_id) {
      skipNextSessionLoad = true;
      activeSessionId.value = res.session_id;
      void loadSessions();
    }

    // 更新 cost signal 供 TokenChip 展示真实费率
    if (res.cost_estimate) {
      lastCostEstimate.value = res.cost_estimate;
    }

    if (res.hint) {
      // hint 作为一条 system 消息追加（暂时；Phase 6 会改为顶部 banner）
      messages.value = [
        ...messages.value,
        {
          id: `s-${Date.now()}`,
          role: 'system',
          content: res.hint,
          created_at: new Date().toISOString(),
        },
      ];
    }
  } catch (e) {
    // 超时（abort）与一般失败分别给文案；都暴露重试入口（lastFailedSend）让用户恢复。
    const content = timedOut
      ? `⚠ ${t('chat.error.timeout')}`
      : `⚠ ${t('chat.error.send_failed', { message: e instanceof Error ? e.message : String(e) })}`;
    const errMsg: Message = {
      id: `err-${Date.now()}`,
      role: 'system',
      content,
      created_at: new Date().toISOString(),
    };
    messages.value = [...messages.value, errMsg];
    lastFailedSend.value = trimmed; // ChatView 据此显示"重试"
  } finally {
    sendInFlight = false;
  }
}

// ── 工具 ─────────────────────────────────────────────────────

function normalizeLocalSchedulerStatus(status: string | null | undefined): string {
  return (status ?? '').trim().toLowerCase();
}

function isLocalSchedulerSuccessStatus(status: string | null | undefined): boolean {
  switch (normalizeLocalSchedulerStatus(status)) {
    case 'done':
    case 'completed':
    case 'complete':
    case 'success':
    case 'succeeded':
      return true;
    default:
      return false;
  }
}

function isLocalSchedulerFailureStatus(status: string | null | undefined): boolean {
  switch (normalizeLocalSchedulerStatus(status)) {
    case 'error':
    case 'failed':
    case 'failure':
    case 'canceled':
    case 'cancelled':
    case 'expired':
      return true;
    default:
      return false;
  }
}

function isLocalSchedulerTerminalStatus(status: string | null | undefined): boolean {
  return isLocalSchedulerSuccessStatus(status) || isLocalSchedulerFailureStatus(status);
}

function mergeSchedulerJobStatus(
  current: LocalSchedulerInfo | undefined,
  job: SchedulerJobStatus,
): LocalSchedulerInfo {
  return {
    ...current,
    task: job.task ?? current?.task,
    scheduled_as: job.scheduled_as ?? current?.scheduled_as,
    job_id: job.job_id ?? current?.job_id,
    status: job.status ?? current?.status,
    phase: job.phase ?? current?.phase,
    reason: job.reason ?? current?.reason,
    eta_ms: job.eta_ms ?? current?.eta_ms,
    model: job.model ?? current?.model,
    service_class: job.service_class ?? current?.service_class,
    device_used: job.device_used ?? current?.device_used,
    latency_ms: job.latency_ms ?? current?.latency_ms,
    queue_wait_ms: job.queue_wait_ms ?? current?.queue_wait_ms,
    error: job.error ?? current?.error,
    detail: job.detail ?? current?.detail,
    admission: current?.admission,
  };
}

function patchMessageById(
  messageId: string,
  update: (message: Message) => Message,
): boolean {
  let found = false;
  const next = messages.value.map((m) => {
    if (m.id !== messageId) return m;
    found = true;
    return update(m);
  });
  if (found) messages.value = next;
  return found;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function nonEmptyString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

function extractLocalSchedulerOutputText(outputs: unknown): string | null {
  const direct = nonEmptyString(outputs);
  if (direct) return direct;
  if (!isRecord(outputs)) return null;

  for (const key of ['answer', 'text', 'content', 'response', 'summary', 'output']) {
    const value = nonEmptyString(outputs[key]);
    if (value) return value;
  }

  const choices = outputs.choices;
  if (!Array.isArray(choices)) return null;
  const first = choices[0];
  if (!isRecord(first)) return null;
  const text = nonEmptyString(first.text);
  if (text) return text;
  const message = first.message;
  if (!isRecord(message)) return null;
  return nonEmptyString(message.content);
}

function localSchedulerFailureContent(job: SchedulerJobStatus): string {
  const status = normalizeLocalSchedulerStatus(job.status);
  if (status === 'canceled' || status === 'cancelled') {
    return t('chat.local_scheduler.canceled');
  }
  if (status === 'expired') {
    return t('chat.local_scheduler.expired');
  }
  const message = schedulerErrorText(job.error) ?? job.detail ?? job.reason ?? job.status ?? 'unknown';
  return t('chat.local_scheduler.failed', { message });
}

function schedulerErrorText(value: unknown): string | null {
  if (typeof value === 'string' && value.trim()) return value;
  if (!isRecord(value)) return null;
  for (const key of ['message', 'error', 'detail', 'reason', 'code']) {
    const text = nonEmptyString(value[key]);
    if (text) return text;
  }
  try {
    return JSON.stringify(value);
  } catch {
    return null;
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function pollLocalSchedulerJob(messageId: string, jobId: string): Promise<void> {
  for (let attempt = 0; attempt < LOCAL_SCHEDULER_JOB_MAX_POLLS; attempt++) {
    await sleep(LOCAL_SCHEDULER_JOB_POLL_INTERVAL_MS);
    if (!messages.value.some((m) => m.id === messageId)) return;

    let job: SchedulerJobStatus;
    try {
      const res = await getSchedulerJob(jobId);
      job = res.job;
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      patchMessageById(messageId, (m) => ({
        ...m,
        content: t('chat.local_scheduler.poll_failed', { message }),
        local_scheduler: {
          ...m.local_scheduler,
          job_id: jobId,
          status: 'poll_error',
          error: message,
        },
      }));
      return;
    }

    const stillPresent = patchMessageById(messageId, (m) => ({
      ...m,
      local_scheduler: mergeSchedulerJobStatus(m.local_scheduler, job),
    }));
    if (!stillPresent) return;

    if (isLocalSchedulerSuccessStatus(job.status)) {
      const content = extractLocalSchedulerOutputText(job.outputs) ?? t('chat.local_scheduler.done_without_text');
      patchMessageById(messageId, (m) => ({
        ...m,
        content,
        local_scheduler: mergeSchedulerJobStatus(m.local_scheduler, job),
      }));
      return;
    }

    if (isLocalSchedulerFailureStatus(job.status)) {
      patchMessageById(messageId, (m) => ({
        ...m,
        content: localSchedulerFailureContent(job),
        local_scheduler: mergeSchedulerJobStatus(m.local_scheduler, job),
      }));
      return;
    }
  }

  patchMessageById(messageId, (m) => ({
    ...m,
    content: t('chat.local_scheduler.poll_timeout'),
    local_scheduler: {
      ...m.local_scheduler,
      job_id: jobId,
      status: 'poll_timeout',
    },
  }));
}

/** 简单 token 估算：中文 ~1 token/字，英文 ~0.25 token/字 */
export function estimateTokens(text: string): number {
  if (!text) return 0;
  let cn = 0;
  let other = 0;
  for (const ch of text) {
    if (/[\u4e00-\u9fff\u3000-\u303f\uff00-\uffef]/.test(ch)) cn++;
    else other++;
  }
  return Math.round(cn + other / 4);
}

export async function deleteSession(sessionId: string): Promise<void> {
  try {
    await api.delete(`/chat/sessions/${sessionId}`);
    if (activeSessionId.value === sessionId) clearActiveSession();
    await loadSessions();
  } catch {
    // 外部 toast 由调用方处理
  }
}
