/** useChat · Chat 核心逻辑封装
 * 见 spec §6 "API 契约" + §5 "business signals"
 *
 * 职责：
 *   - 加载 / 刷新 session 列表
 *   - 加载某 session 的消息历史
 *   - 发送消息（optimistic + 后台 API + 自动回填 session_id）
 *   - 新建 / 删除 session
 */

import { api } from '../store/api';
import {
  chatSessions,
  activeSessionId,
  messages,
  lastCostEstimate,
} from '../store/signals';
import type { ChatSession, Message, CostEstimate } from '../store/signals';

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
};

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

  try {
    const body: Record<string, unknown> = {
      message: trimmed,
      history,
    };
    if (currentSession) body.session_id = currentSession;

    const res = await api.post<ChatResponse>('/chat', body);

    const assistantMsg: Message = {
      id: `a-${Date.now()}`,
      role: 'assistant',
      content: res.content,
      citations: res.citations,
      created_at: new Date().toISOString(),
    };
    messages.value = [...messages.value, assistantMsg];

    // 若是第一次发送（无 session_id），回填 + 刷新列表
    if (!currentSession && res.session_id) {
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
    // 失败 → 系统消息提示
    const errMsg: Message = {
      id: `err-${Date.now()}`,
      role: 'system',
      content: `⚠ 发送失败：${e instanceof Error ? e.message : String(e)}`,
      created_at: new Date().toISOString(),
    };
    messages.value = [...messages.value, errMsg];
  } finally {
    sendInFlight = false;
  }
}

// ── 工具 ─────────────────────────────────────────────────────

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
