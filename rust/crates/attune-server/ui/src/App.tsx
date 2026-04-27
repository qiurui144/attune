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
import { ToastContainer, RecommendationOverlay } from './components';
import { CommandPalette } from './components/CommandPalette';
import { Wizard, LoginScreen } from './wizard';
import { MainShell } from './layout';
import { useShortcut } from './hooks/useShortcut';
import { api, ApiError } from './store/api';
import { vaultState, sidebarCollapsed } from './store/signals';
import { startConnectionMonitor } from './store/connection';
import { startProgressWS } from './store/ws';

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

export function App(): JSX.Element {
  const phase = useSignal<AppPhase>({ kind: 'booting' });
  const paletteOpen = useSignal(false);
  const [bootError, setBootError] = useState<string | null>(null);

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
    startProgressWS();
    void bootstrap();
  }, []);

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
    phase.value = { kind: 'main' };
  }

  async function handleUnlock() {
    vaultState.value = 'unlocked';
    const settings = await api.get<SettingsResponse>('/settings').catch(() => ({}) as SettingsResponse);
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
            启动失败
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
            重试
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
        <ToastContainer />
      </>
    );
  }

  // Phase 4+：Main 布局（Sidebar + Views + Drawer + CommandPalette）
  return (
    <>
      <MainShell />
      <CommandPalette open={paletteOpen.value} onClose={() => (paletteOpen.value = false)} />
      <RecommendationOverlay />
      <ToastContainer />
    </>
  );
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
          Attune 正在启动…
        </span>
      </div>
    </div>
  );
}

