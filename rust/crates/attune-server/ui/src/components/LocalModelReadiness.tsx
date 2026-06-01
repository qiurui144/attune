/**
 * LocalModelReadiness — 本地模型一键就绪面板 (zero-terminal UX)
 *
 * 面向非技术用户：把 Ollama "守护进程是否在 + 模型是否已下载" 归一成三态，
 * 每个缺失态都配一键修复按钮（永不让用户去终端敲命令）：
 *   🔴 daemon_down   → 一键安装 Ollama（POST /ollama/install）+ 轮询直到 daemon 起来
 *   🟡 model_missing → 一键拉取模型（POST /models/pull）+ 轮询直到模型就绪
 *   🟢 ready         → 绿色就绪提示
 *
 * 装不了的平台（macOS / 未知）graceful 降级：给官网下载链接（download_url）。
 */

import type { JSX } from 'preact';
import { useState, useEffect, useCallback } from 'preact/hooks';
import { Button } from './Button';
import { toast } from './Toast';
import { t } from '../i18n';
import { api } from '../store/api';

type ReadinessState = 'daemon_down' | 'model_missing' | 'ready';

type InstallMethodKind = 'script' | 'installer' | 'manual_download';

type ReadinessResponse = {
  readiness: {
    state: ReadinessState;
    configured?: string;
    available?: string[];
    resolved?: string;
  };
  models: string[];
  install_plan: {
    platform: string;
    method: { kind: InstallMethodKind; command?: string; download_url?: string };
    homepage: string;
  };
};

type InstallResponse = {
  status: 'queued' | 'manual' | 'busy';
  task_id?: string | null;
  download_url?: string | null;
  message: string;
};

export type LocalModelReadinessProps = {
  /** 要核对的 Ollama 模型 tag（如 qwen2.5:3b）。空 = 只看 daemon 是否在。 */
  model: string;
  /** 就绪态变化回调（父组件据此 enable/disable "使用本地" 按钮）。 */
  onReadyChange?: (ready: boolean) => void;
  /** 紧凑模式（Settings tab 内嵌时用更小字号）。 */
  compact?: boolean;
};

const POLL_INTERVAL_MS = 4000;
const POLL_MAX = 75; // 75 × 4s = 5min 上限（安装/大模型拉取够用）

export function LocalModelReadiness({
  model,
  onReadyChange,
  compact,
}: LocalModelReadinessProps): JSX.Element {
  const [resp, setResp] = useState<ReadinessResponse | null>(null);
  const [checking, setChecking] = useState(true);
  const [busy, setBusy] = useState(false); // 安装/拉取进行中
  const [busyLabel, setBusyLabel] = useState('');
  const [polling, setPolling] = useState(false);

  const probe = useCallback(async (): Promise<ReadinessResponse | null> => {
    try {
      const q = model.trim() ? `?model=${encodeURIComponent(model.trim())}` : '';
      const r = await api.get<ReadinessResponse>(`/ollama/readiness${q}`);
      setResp(r);
      onReadyChange?.(r.readiness.state === 'ready');
      return r;
    } catch {
      // 探测失败按 daemon_down 处理（graceful，不报红）
      setResp(null);
      onReadyChange?.(false);
      return null;
    }
  }, [model, onReadyChange]);

  useEffect(() => {
    let alive = true;
    setChecking(true);
    void probe().finally(() => {
      if (alive) setChecking(false);
    });
    return () => {
      alive = false;
    };
  }, [probe]);

  /** 轮询直到目标态达成或超时。 */
  const pollUntilReady = useCallback(
    async (target: ReadinessState | 'daemon_up') => {
      setPolling(true);
      for (let i = 0; i < POLL_MAX; i++) {
        await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
        const r = await probe();
        if (!r) continue;
        const s = r.readiness.state;
        if (target === 'daemon_up' && s !== 'daemon_down') {
          setPolling(false);
          return true;
        }
        if (target === 'ready' && s === 'ready') {
          setPolling(false);
          return true;
        }
      }
      setPolling(false);
      return false;
    },
    [probe],
  );

  async function handleInstall() {
    const plan = resp?.install_plan;
    if (!plan) return;
    // 不可应用内安装的平台（macOS/未知）→ 打开官网下载页（graceful 降级）。
    const kind = plan.method.kind;
    if (kind !== 'script') {
      const url = plan.method.download_url ?? plan.homepage;
      window.open(url, '_blank', 'noopener');
      toast('info', t('local_model.install.manual_opened'));
      return;
    }
    setBusy(true);
    setBusyLabel(t('local_model.install.installing'));
    try {
      const r = await api.post<InstallResponse>('/ollama/install', {});
      if (r.status === 'manual') {
        const url = r.download_url ?? plan.homepage;
        window.open(url, '_blank', 'noopener');
        toast('info', r.message);
        setBusy(false);
        return;
      }
      if (r.status === 'busy') {
        toast('info', r.message);
        setBusy(false);
        return;
      }
      toast('success', r.message);
      const ok = await pollUntilReady('daemon_up');
      if (!ok) toast('error', t('local_model.install.timeout'));
    } catch (e) {
      toast('error', t('local_model.install.failed', { message: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(false);
    }
  }

  async function handlePull() {
    if (!model.trim()) return;
    setBusy(true);
    setBusyLabel(t('local_model.pull.pulling', { model: model.trim() }));
    try {
      await api.post<{ task_id: string; status: string }>('/models/pull', { model: model.trim() });
      toast('success', t('local_model.pull.started', { model: model.trim() }));
      const ok = await pollUntilReady('ready');
      if (ok) toast('success', t('local_model.pull.done', { model: model.trim() }));
      else toast('error', t('local_model.pull.timeout'));
    } catch (e) {
      toast('error', t('local_model.pull.failed', { message: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(false);
    }
  }

  const fontSize = compact ? 'var(--text-xs)' : 'var(--text-sm)';
  const state = resp?.readiness.state ?? 'daemon_down';

  if (checking) {
    return (
      <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)', fontSize }}>
        <span className="spinner" />
        {t('local_model.checking')}
      </div>
    );
  }

  if (busy || polling) {
    return (
      <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)', fontSize }}>
        <span className="spinner" />
        {busyLabel || t('local_model.working')}
      </div>
    );
  }

  if (state === 'ready') {
    const resolved = resp?.readiness.resolved;
    return (
      <div style={{ color: 'var(--color-success)', fontSize }}>
        {resolved
          ? t('local_model.ready_with_model', { model: resolved })
          : t('local_model.ready')}
      </div>
    );
  }

  if (state === 'daemon_down') {
    const autoInstallable = resp?.install_plan.method.kind === 'script';
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)', fontSize }}>
        <div style={{ color: 'var(--color-warning)' }}>🔴 {t('local_model.daemon_down')}</div>
        <Button size="sm" variant="primary" onClick={handleInstall}>
          {autoInstallable ? t('local_model.install.btn') : t('local_model.install.btn_download')}
        </Button>
      </div>
    );
  }

  // model_missing
  const configured = resp?.readiness.configured ?? model;
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)', fontSize }}>
      <div style={{ color: 'var(--color-warning)' }}>
        🟡 {t('local_model.model_missing', { model: configured })}
      </div>
      <Button size="sm" variant="primary" onClick={handlePull}>
        {t('local_model.pull.btn', { model: configured })}
      </Button>
    </div>
  );
}
