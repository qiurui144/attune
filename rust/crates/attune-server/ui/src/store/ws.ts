/** WebSocket 通用 progress 通道（/ws/scan-progress）
 * 见 spec §6 WebSocket 扩展
 *
 * 自动重连（指数回退 500 → 30s），写入 backgroundTasks signal 供 sidebar 显示。
 */

import {
  backgroundTasks,
  pushRecommendation,
} from './signals';
import type {
  BackgroundTask,
  RecommendationPayload,
  WorkflowCompletePayload,
} from './signals';
import { toast } from '../components';

let ws: WebSocket | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let stopped = false;
let backoff = 500;
const MAX_BACKOFF = 30_000;

export function startProgressWS(): void {
  stopProgressWS();
  stopped = false;
  connect();
}

export function stopProgressWS(): void {
  stopped = true;
  if (reconnectTimer !== null) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  if (ws) {
    ws.onclose = null;
    ws.onerror = null;
    ws.close();
    ws = null;
  }
}

function connect(): void {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const url = `${proto}://${location.host}/ws/scan-progress`;
  try {
    ws = new WebSocket(url);
  } catch {
    scheduleReconnect();
    return;
  }
  ws.onopen = () => {
    backoff = 500;
  };
  ws.onmessage = (ev) => {
    try {
      const payload = JSON.parse(ev.data);
      dispatch(payload);
    } catch {
      /* ignore malformed */
    }
  };
  ws.onclose = scheduleReconnect;
  ws.onerror = () => {
    ws?.close();
  };
}

function scheduleReconnect(): void {
  if (stopped) return;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    if (!stopped) connect();
  }, backoff);
  backoff = Math.min(backoff * 2, MAX_BACKOFF);
}

/**
 * Sprint 1 Phase D-2: 按 payload.type 分流。
 *
 * - "progress"               → applyProgress（沿用旧逻辑）
 * - "project_recommendation" → 推入 recommendations 信号，UI 出右下角气泡
 * - "workflow_complete"      → 走 toast 系统短暂提示
 * - 无 type 字段             → 兼容旧汇总格式（pending_embeddings…）
 */
function dispatch(payload: Record<string, unknown>): void {
  const ty = typeof payload.type === 'string' ? payload.type : undefined;
  if (ty === 'project_recommendation') {
    pushRecommendation(payload as unknown as RecommendationPayload);
    return;
  }
  if (ty === 'workflow_complete') {
    const wf = payload as unknown as WorkflowCompletePayload;
    const fileTail =
      typeof wf.file_id === 'string' && wf.file_id.length >= 8
        ? wf.file_id.slice(0, 8)
        : (wf.file_id ?? '');
    toast(
      'success',
      `工作流 ${wf.workflow_id} 已完成 · file ${fileTail}`,
      5000,
    );
    return;
  }
  // type === 'progress' 或无 type → 旧 progress 处理路径
  applyProgress(payload);
}

function applyProgress(payload: Record<string, unknown>): void {
  // 旧格式：{ pending_embeddings, pending_classify, … } · 汇总显示
  // 新格式（future）：{ type, task_id, progress, status, message }
  if (typeof payload.type === 'string' && typeof payload.task_id === 'string') {
    const task: BackgroundTask = {
      type: payload.type,
      task_id: String(payload.task_id),
      progress: typeof payload.progress === 'number' ? payload.progress : 0,
      status:
        payload.status === 'done' || payload.status === 'failed'
          ? payload.status
          : 'running',
      message: typeof payload.message === 'string' ? payload.message : '',
    };
    const list = backgroundTasks.value.filter((t) => t.task_id !== task.task_id);
    if (task.status === 'done' || task.status === 'failed') {
      // 完成的任务 5s 后移除
      backgroundTasks.value = [...list, task];
      setTimeout(() => {
        backgroundTasks.value = backgroundTasks.value.filter(
          (t) => t.task_id !== task.task_id,
        );
      }, 5_000);
    } else {
      backgroundTasks.value = [...list, task];
    }
    return;
  }

  // 旧格式：汇总成一个虚拟 task
  const pending =
    (typeof payload.pending_embeddings === 'number' ? payload.pending_embeddings : 0) +
    (typeof payload.pending_classify === 'number' ? payload.pending_classify : 0);
  if (pending > 0) {
    const existing = backgroundTasks.value.find((t) => t.task_id === 'legacy-queue');
    const task: BackgroundTask = {
      type: 'legacy',
      task_id: 'legacy-queue',
      progress: existing?.progress ?? 0,
      status: 'running',
      message: `${pending} 个后台任务`,
    };
    const list = backgroundTasks.value.filter((t) => t.task_id !== 'legacy-queue');
    backgroundTasks.value = [...list, task];
  } else {
    backgroundTasks.value = backgroundTasks.value.filter((t) => t.task_id !== 'legacy-queue');
  }
}
