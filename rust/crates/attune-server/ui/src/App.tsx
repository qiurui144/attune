/** Attune 主应用根组件（Phase 3 · wizard 路由就位）
 *
 * 启动流：
 *   1. 读 /vault/status → vaultState
 *   2. 读 /settings → wizardState（若存在）
 *   3. 按矩阵路由：
 *      - sealed           → Wizard (Step 1 Welcome)
 *      - locked           → LoginScreen
 *      - unlocked + !wizard.complete → 回到 Wizard.current_step
 *      - unlocked + wizard.complete  → MainApp
 *
 * 下一 Phase（4）：MainApp 里接入 Sidebar + Chat view
 */

import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, ToastContainer, RecommendationOverlay, ConfirmHost } from './components';
import { toast } from './components/Toast';
import { CommandPalette } from './components/CommandPalette';
import { Wizard, LoginScreen } from './wizard';
import { MainShell } from './layout';
import { PrivacyTour } from './views/PrivacyTour';
import { useShortcut } from './hooks/useShortcut';
import { api, ApiError, clearToken, getToken } from './store/api';
import {
  currentView,
  memberState,
  settings as appSettings,
  settingsInitialTab,
  vaultState,
  sidebarCollapsed,
} from './store/signals';
import type { SettingsTabId, View } from './store/signals';
import { startConnectionMonitor } from './store/connection';
import { startProgressWS } from './store/ws';
import { loadMemberState } from './hooks/useMember';
import { loadSettings } from './hooks/useSettings';
import { t } from './i18n';

type VaultStatusResponse = {
  state: 'sealed' | 'locked' | 'unlocked';
  items?: number;
};

type SettingsResponse = {
  wizard?: {
    complete?: boolean;
    current_step?: number;
  };
};

type AppPhase =
  | { kind: 'booting' }
  | { kind: 'wizard' }
  | { kind: 'login' }
  | { kind: 'main' };

type UpdateNotice = {
  state: 'available' | 'downloading' | 'ready' | 'error';
  from?: string;
  to?: string;
  percent?: number;
  message?: string;
};

export function App(): JSX.Element {
  const phase = useSignal<AppPhase>({ kind: 'booting' });
  const paletteOpen = useSignal(false);
  const [bootError, setBootError] = useState<string | null>(null);
  const [updateNotice, setUpdateNotice] = useState<UpdateNotice | null>(null);

  // Minor 3.4 修复：theme attribute 已经在 store/signals.ts 的 subscribe 里写过了，
  // 这里移除重复写入避免双源。
  // 全局快捷键：⌘K 打开 palette，⌘B 折叠 sidebar
  useShortcut({
    key: 'k',
    meta: true,
    when: () => phase.value.kind === 'main',
    handler: () => (paletteOpen.value = true),
    description: 'shortcut.search',
  });
  useShortcut({
    key: 'b',
    meta: true,
    when: () => phase.value.kind === 'main',
    handler: () => (sidebarCollapsed.value = !sidebarCollapsed.value),
    description: 'shortcut.toggle_sidebar',
  });

  // 启动
  useEffect(() => {
    startConnectionMonitor();
    // B2: only open the scan-progress WS once a session token exists. Connecting
    // pre-auth (no vault / locked) just spams `/ws/scan-progress 401` + reconnects.
    // handleUnlock() and handleWizardComplete() re-call startProgressWS() once the
    // token is set, so the live cases are still covered.
    if (getToken() != null) startProgressWS();
    void bootstrap();

    // Tauri 桌面模式:监听 OS 文件拖拽 → 调 upload_dropped_paths 上传。
    // 经 @tauri-apps/api(__TAURI_INTERNALS__ 始终注入);不用 window.__TAURI__
    // (withGlobalTauri 默认 false,那条路径不存在 → 旧实现是死代码)。
    const unlisteners: Array<() => void> = [];
    if (typeof window !== 'undefined' && (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) {
      void (async () => {
        try {
          const [{ listen }, { invoke }] = await Promise.all([
            import('@tauri-apps/api/event'),
            import('@tauri-apps/api/core'),
          ]);
          unlisteners.push(await listen<string[]>('attune-file-drop', async (event) => {
            const paths = event.payload ?? [];
            if (paths.length === 0) return;
            try {
              const results = await invoke<string[]>('upload_dropped_paths', { paths });
              const ok = results.filter((r) => r.startsWith('ok:')).length;
              const failed = results.length - ok;
              if (ok > 0) toast('success', t('app.tauri.upload_ok', { count: ok }));
              if (failed > 0) toast('error', t('app.tauri.upload_fail', { count: failed }));
            } catch (e) {
              toast('error', e instanceof Error ? e.message : t('app.tauri.upload_err'));
            }
          }));
          unlisteners.push(await listen<{ view?: string; settingsTab?: string }>('attune-navigate', (event) => {
            const view = event.payload?.view;
            const tab = event.payload?.settingsTab;
            if (isView(view)) currentView.value = view;
            if (isSettingsTab(tab)) settingsInitialTab.value = tab;
          }));
          unlisteners.push(await listen('attune-lock-vault', async () => {
            await lockVaultFromShell();
          }));
          unlisteners.push(await listen<UpdateNotice>('attune-update-status', (event) => {
            const payload = event.payload;
            switch (payload?.state) {
              case 'available':
                setUpdateNotice(payload);
                toast('info', t('app.update.available_toast'));
                break;
              case 'downloading':
                setUpdateNotice(payload);
                break;
              case 'ready':
                setUpdateNotice(payload);
                toast('success', t('app.update.ready_toast'));
                break;
              case 'error':
                setUpdateNotice(payload);
                toast('error', t('app.update.error_toast'));
                break;
            }
          }));
          unlisteners.push(await listen('attune-window-hidden', () => {
            const key = 'attune.close_to_tray.hint_seen';
            if (localStorage.getItem(key) !== '1') {
              localStorage.setItem(key, '1');
              toast('info', t('app.tray.hidden_toast'));
            }
          }));
        } catch (e) {
          console.warn('failed to attach Tauri desktop listeners:', e);
        }
      })();
    }

    let autoLockTimer: number | null = null;
    let autoLockAuditTimer: number | null = null;
    let lastActivityAt = Date.now();
    const clearAutoLockTimer = (): void => {
      if (autoLockTimer !== null) {
        window.clearTimeout(autoLockTimer);
        autoLockTimer = null;
      }
    };
    const autoLockNow = async (): Promise<void> => {
      if (vaultState.value !== 'unlocked') return;
      try {
        await api.post('/vault/lock');
        clearToken();
        vaultState.value = 'locked';
        toast('info', t('app.security.auto_locked'));
        window.location.reload();
      } catch {
        clearAutoLockTimer();
      }
    };
    const scheduleAutoLock = (): void => {
      clearAutoLockTimer();
      const raw = localStorage.getItem('attune.security.auto_lock_minutes') ?? '0';
      const minutes = Number(raw);
      if (!Number.isFinite(minutes) || minutes <= 0 || vaultState.value !== 'unlocked') return;
      const timeoutMs = minutes * 60 * 1000;
      const remainingMs = timeoutMs - (Date.now() - lastActivityAt);
      if (remainingMs <= 0) {
        void autoLockNow();
        return;
      }
      autoLockTimer = window.setTimeout(() => {
        void autoLockNow();
      }, remainingMs);
    };
    const recordActivity = (): void => {
      lastActivityAt = Date.now();
      scheduleAutoLock();
    };
    const activityEvents = ['pointerdown', 'keydown', 'wheel', 'touchstart'];
    for (const event of activityEvents) window.addEventListener(event, recordActivity, { passive: true });
    window.addEventListener('attune-auto-lock-config-changed', scheduleAutoLock);
    window.addEventListener('focus', scheduleAutoLock);
    window.addEventListener('pageshow', scheduleAutoLock);
    document.addEventListener('visibilitychange', scheduleAutoLock);
    autoLockAuditTimer = window.setInterval(scheduleAutoLock, 60_000);
    scheduleAutoLock();

    return () => {
      for (const unlisten of unlisteners) unlisten();
      for (const event of activityEvents) window.removeEventListener(event, recordActivity);
      window.removeEventListener('attune-auto-lock-config-changed', scheduleAutoLock);
      window.removeEventListener('focus', scheduleAutoLock);
      window.removeEventListener('pageshow', scheduleAutoLock);
      document.removeEventListener('visibilitychange', scheduleAutoLock);
      if (autoLockAuditTimer !== null) window.clearInterval(autoLockAuditTimer);
      clearAutoLockTimer();
    };
  }, []);

  useEffect(() => {
    if (phase.value.kind !== 'main') return;
    void loadMemberState();
    const id = window.setInterval(() => {
      void loadMemberState();
    }, 60_000);
    return () => window.clearInterval(id);
  }, [phase.value.kind]);

  async function lockVaultFromShell(): Promise<void> {
    try {
      await api.post('/vault/lock');
      clearToken();
      vaultState.value = 'locked';
      window.location.reload();
    } catch (e) {
      toast('error', e instanceof Error ? e.message : t('sidebar.menu.lock_vault.error'));
    }
  }

  async function bootstrap() {
    try {
      const status = await api.get<VaultStatusResponse>('/vault/status');
      vaultState.value = status.state;

      if (status.state === 'sealed') {
        phase.value = { kind: 'wizard' };
        return;
      }

      if (status.state === 'locked') {
        phase.value = { kind: 'login' };
        return;
      }

      // unlocked → 检查 wizard 是否完成
      // 注意：unlocked + 401 说明服务端 vault 已解锁但客户端没有有效 session token
      // （浏览器重启 / token 过期）。此时必须跳 LoginScreen 让用户重新输入密码取 token，
      // 否则后续所有 API 调用都会 401 失败。不能把 401 catch 成空对象然后误判 wizard 未完成。
      let settings: SettingsResponse;
      try {
        settings = await api.get<SettingsResponse>('/settings');
        appSettings.value = settings as Record<string, unknown>;
      } catch (err) {
        if (err instanceof ApiError && err.status === 401) {
          phase.value = { kind: 'login' };
          return;
        }
        // 其他错误（网络/5xx）→ 回退为空 settings，按 wizard 未完成处理
        settings = {};
      }
      if (settings.wizard?.complete) {
        phase.value = { kind: 'main' };
      } else {
        // unlocked 但 wizard 未完成 → 回到 wizard
        phase.value = { kind: 'wizard' };
      }
    } catch (e) {
      setBootError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleWizardComplete() {
    // 标记 wizard 已完成
    try {
      await api.patch('/settings', {
        wizard: { complete: true },
      });
    } catch {
      /* 失败不阻塞，下次启动仍会跳回 wizard */
    }
    // Reconnect the progress WebSocket now that a token is available.
    startProgressWS();
    await loadSettings();
    phase.value = { kind: 'main' };
  }

  async function handleUnlock() {
    vaultState.value = 'unlocked';
    // Reconnect the progress WebSocket now that a token is available.
    startProgressWS();
    const settings = await api.get<SettingsResponse>('/settings').catch(() => ({}) as SettingsResponse);
    appSettings.value = settings as Record<string, unknown>;
    if (settings.wizard?.complete) {
      phase.value = { kind: 'main' };
    } else {
      phase.value = { kind: 'wizard' };
    }
  }

  if (bootError) {
    return (
      <div
        style={{
          minHeight: '100vh',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          padding: 'var(--space-5)',
          textAlign: 'center',
        }}
      >
        <div style={{ maxWidth: 400 }}>
          <div style={{ fontSize: 48, marginBottom: 'var(--space-3)' }}>⚠</div>
          <h1 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, marginBottom: 'var(--space-2)' }}>
            {t('app.boot.failed')}
          </h1>
          <p
            style={{
              fontSize: 'var(--text-sm)',
              color: 'var(--color-text-secondary)',
              marginBottom: 'var(--space-4)',
            }}
          >
            {bootError}
          </p>
          <button
            type="button"
            onClick={() => {
              setBootError(null);
              phase.value = { kind: 'booting' };
              void bootstrap();
            }}
            style={{
              padding: 'var(--space-2) var(--space-4)',
              background: 'var(--color-accent)',
              color: 'white',
              border: 'none',
              borderRadius: 'var(--radius-md)',
              cursor: 'pointer',
            }}
          >
            {t('common.retry')}
          </button>
        </div>
        <ToastContainer />
      </div>
    );
  }

  if (phase.value.kind === 'booting') {
    return <BootingSplash />;
  }

  if (phase.value.kind === 'wizard') {
    return (
      <>
        <Wizard onComplete={handleWizardComplete} />
        <ToastContainer />
      </>
    );
  }

  if (phase.value.kind === 'login') {
    return (
      <>
        <LoginScreen onUnlock={handleUnlock} />
        <ConfirmHost />
        <ToastContainer />
      </>
    );
  }

  // Phase 4+：Main 布局（Sidebar + Views + Drawer + CommandPalette）
  return (
    <>
      <MainShell />
      <DesktopUpdateBanner notice={updateNotice} onDismiss={() => setUpdateNotice(null)} />
      <MemberQuotaBanner />
      <PrivacyTour />
      <CommandPalette open={paletteOpen.value} onClose={() => (paletteOpen.value = false)} />
      <RecommendationOverlay />
      <ConfirmHost />
      <ToastContainer />
    </>
  );
}

function MemberQuotaBanner(): JSX.Element | null {
  const m = memberState.value;
  if (!m) return null;
  const quota = m.llm_quota_remaining ?? 0;
  const quotaText = quota > 0 ? quota.toLocaleString() : '';
  const isPaid = m.kind === 'paid';
  const label = isPaid
    ? quota > 0
      ? t('member.banner.paid_quota', { quota: quotaText })
      : t('member.banner.paid')
    : m.kind === 'free'
      ? t('member.banner.free')
      : t('member.banner.logged_out');
  const borderColor = isPaid && quota > 0 && quota < 10_000 ? 'var(--color-warning)' : 'var(--color-border)';

  function openQuota(): void {
    currentView.value = 'quota';
  }

  function openMember(): void {
    settingsInitialTab.value = 'member';
    currentView.value = 'settings';
  }

  return (
    <div
      role="status"
      style={{
        position: 'fixed',
        right: 'var(--space-4)',
        bottom: 'var(--space-4)',
        zIndex: 1200,
        width: 'min(420px, calc(100vw - 32px))',
        padding: 'var(--space-2)',
        background: 'var(--color-surface)',
        border: `1px solid ${borderColor}`,
        borderRadius: 'var(--radius-md)',
        boxShadow: 'var(--shadow-md)',
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-2)',
      }}
    >
      <div
        style={{
          minWidth: 0,
          flex: 1,
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-secondary)',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          whiteSpace: 'nowrap',
        }}
      >
        {label}
      </div>
      <Button size="sm" variant="secondary" onClick={openQuota}>
        {t('member.banner.quota')}
      </Button>
      {!isPaid && (
        <Button size="sm" variant="primary" onClick={openMember}>
          {t('member.banner.member')}
        </Button>
      )}
    </div>
  );
}

function DesktopUpdateBanner({
  notice,
  onDismiss,
}: {
  notice: UpdateNotice | null;
  onDismiss: () => void;
}): JSX.Element | null {
  if (!notice) return null;

  const title = (() => {
    switch (notice.state) {
      case 'available': return t('app.update.banner.available');
      case 'downloading': return t('app.update.banner.downloading', { percent: notice.percent ?? 0 });
      case 'ready': return t('app.update.banner.ready');
      case 'error': return t('app.update.banner.error');
    }
  })();
  const detail = notice.to
    ? t('app.update.banner.version', { from: notice.from ?? '-', to: notice.to })
    : notice.message ?? '';

  async function restart(): Promise<void> {
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('restart_for_update');
    } catch (e) {
      toast('error', e instanceof Error ? e.message : t('app.update.error_toast'));
    }
  }

  function openUpdateSettings(): void {
    settingsInitialTab.value = 'about';
    currentView.value = 'settings';
  }

  return (
    <div
      role={notice.state === 'error' ? 'alert' : 'status'}
      style={{
        position: 'fixed',
        top: 'var(--space-4)',
        right: 'var(--space-4)',
        zIndex: 1900,
        width: 'min(520px, calc(100vw - 32px))',
        padding: 'var(--space-3)',
        background: 'var(--color-surface)',
        border: `1px solid ${notice.state === 'error' ? 'var(--color-error)' : 'var(--color-accent)'}`,
        borderRadius: 'var(--radius-md)',
        boxShadow: 'var(--shadow-lg)',
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-3)',
      }}
    >
      <div style={{ minWidth: 0, flex: 1 }}>
        <div style={{ fontSize: 'var(--text-sm)', fontWeight: 600, color: 'var(--color-text)' }}>
          {title}
        </div>
        {detail && (
          <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
            {detail}
          </div>
        )}
      </div>
      {notice.state === 'ready' && (
        <Button size="sm" variant="primary" onClick={() => void restart()}>
          {t('app.update.banner.restart')}
        </Button>
      )}
      {notice.state !== 'downloading' && notice.state !== 'ready' && (
        <Button size="sm" variant="secondary" onClick={openUpdateSettings}>
          {t('app.update.banner.settings')}
        </Button>
      )}
      <Button size="sm" variant="ghost" aria-label={t('common.close')} onClick={onDismiss}>
        x
      </Button>
    </div>
  );
}

const KNOWN_VIEWS: readonly View[] = [
  'chat',
  'items',
  'projects',
  'remote',
  'knowledge',
  'skills',
  'marketplace',
  'office',
  'privacy',
  'quota',
  'doc-intel',
  'writing',
  'skill-runner',
  'workbench',
  'monitoring',
  'settings',
];

const SETTINGS_TABS: readonly SettingsTabId[] = ['general', 'ai', 'data', 'plugins', 'member', 'privacy', 'about'];

function isView(value: string | undefined): value is View {
  return typeof value === 'string' && (KNOWN_VIEWS as readonly string[]).includes(value);
}

function isSettingsTab(value: string | undefined): value is SettingsTabId {
  return typeof value === 'string' && (SETTINGS_TABS as readonly string[]).includes(value);
}

function BootingSplash(): JSX.Element {
  return (
    <div
      style={{
        minHeight: '100vh',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 'var(--space-3)',
        background: 'var(--color-bg)',
      }}
    >
      <div style={{ fontSize: 48 }} aria-hidden="true">
        🌿
      </div>
      <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)' }}>
        <span className="spinner" />
        <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>
          {t('app.boot.loading')}
        </span>
      </div>
    </div>
  );
}
