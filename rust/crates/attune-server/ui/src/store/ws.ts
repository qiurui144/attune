/** WebSocket 通用 progress 通道（/ws/scan-progress）
 * 见 spec §6 WebSocket 扩展
 *
 * 自动重连（指数回退 500 → 30s），写入 backgroundTasks signal 供 sidebar 显示。
 */

import { backgroundTasks } from './signals';
import type { BackgroundTask } from './signals';

let ws: WebSocket | null = null;
let backoff = 500;
const MAX_BACKOFF = 30_000;

export function startProgressWS(): void {
  stopProgressWS();
  connect();
}

export function stopProgressWS(): void {
  if (ws) {
    ws.onclose = null;
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
      applyProgress(payload);
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
  setTimeout(connect, backoff);
  backoff = Math.min(backoff * 2, MAX_BACKOFF);
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
