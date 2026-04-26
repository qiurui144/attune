/** Attune Sidebar · 左栏 5 区 · 可折叠
 * 见 spec §4 "Sidebar（左栏 · 5 区）"
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import {
  currentView,
  sidebarCollapsed,
  connectionState,
  chatSessions,
  activeSessionId,
  vaultState,
} from '../store/signals';
import type { View } from '../store/signals';
import { loadSessions, clearActiveSession } from '../hooks/useChat';
import { t } from '../i18n';

const SIDEBAR_WIDTH = 280;
const SIDEBAR_COLLAPSED_WIDTH = 64;

export function Sidebar(): JSX.Element {
  const collapsed = sidebarCollapsed.value;
  const width = collapsed ? SIDEBAR_COLLAPSED_WIDTH : SIDEBAR_WIDTH;

  // 挂载时加载 session 列表
  useEffect(() => {
    void loadSessions();
  }, []);

  return (
    <aside
      aria-label="Navigation"
      style={{
        width,
        flexShrink: 0,
        background: 'var(--color-surface)',
        borderRight: '1px solid var(--color-border)',
        display: 'flex',
        flexDirection: 'column',
        transition: 'width var(--duration-base) var(--ease-out)',
        overflow: 'hidden',
      }}
    >
      <BrandAndSearch collapsed={collapsed} />
      <NewChatButton collapsed={collapsed} />
      <SessionList collapsed={collapsed} />
      <SecondaryNav collapsed={collapsed} />
      <StatusBar collapsed={collapsed} />
    </aside>
  );
}

// ── ① 品牌 + 搜索 ────────────────────────────────────────────
function BrandAndSearch({ collapsed }: { collapsed: boolean }): JSX.Element {
  return (
    <div
      style={{
        padding: 'var(--space-3) var(--space-4)',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-2)',
        borderBottom: '1px solid var(--color-border)',
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: 'var(--space-2)',
        }}
      >
        <span
          style={{
            fontWeight: 600,
            fontSize: 'var(--text-base)',
            color: 'var(--color-text)',
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          🌿 {!collapsed && t('app.name')}
        </span>
        <button
          type="button"
          onClick={() => (sidebarCollapsed.value = !collapsed)}
          aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
          className="interactive"
          style={{
            padding: '4px 6px',
            background: 'transparent',
            border: 'none',
            borderRadius: 'var(--radius-sm)',
            color: 'var(--color-text-secondary)',
            cursor: 'pointer',
            fontSize: 'var(--text-base)',
          }}
        >
          {collapsed ? '»' : '«'}
        </button>
      </div>
      {!collapsed && (
        <button
          type="button"
          aria-label="Global search (Cmd+K)"
          className="interactive"
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 'var(--space-2)',
            padding: '6px var(--space-3)',
            background: 'var(--color-bg)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-md)',
            color: 'var(--color-text-secondary)',
            fontSize: 'var(--text-sm)',
            cursor: 'pointer',
            width: '100%',
            textAlign: 'left',
          }}
          onClick={() => {
            document.dispatchEvent(
              new KeyboardEvent('keydown', { key: 'k', metaKey: true, ctrlKey: true, bubbles: true }),
            );
          }}
        >
          <span aria-hidden="true">🔍</span>
          <span style={{ flex: 1 }}>Search…</span>
          <kbd
            style={{
              fontSize: 'var(--text-xs)',
              padding: '1px 6px',
              background: 'var(--color-surface)',
              border: '1px solid var(--color-border)',
              borderRadius: 'var(--radius-sm)',
              fontFamily: 'var(--font-mono)',
            }}
          >
            ⌘K
          </kbd>
        </button>
      )}
    </div>
  );
}

// ── ② 新对话 CTA ─────────────────────────────────────────────
function NewChatButton({ collapsed }: { collapsed: boolean }): JSX.Element {
  return (
    <div style={{ padding: 'var(--space-3) var(--space-4)' }}>
      <button
        type="button"
        aria-label="New chat"
        onClick={() => {
          clearActiveSession();
          currentView.value = 'chat';
        }}
        className="interactive"
        style={{
          width: '100%',
          height: 'var(--btn-h-md)',
          padding: collapsed ? 0 : '0 var(--space-3)',
          background: 'var(--color-accent)',
          color: 'white',
          border: 'none',
          borderRadius: 'var(--radius-md)',
          fontWeight: 500,
          fontSize: 'var(--text-sm)',
          cursor: 'pointer',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 'var(--space-2)',
        }}
      >
        <span aria-hidden="true">+</span>
        {!collapsed && <span>新对话</span>}
      </button>
    </div>
  );
}

// ── ③ 会话列表（按日期分组） ─────────────────────────────────
function SessionList({ collapsed }: { collapsed: boolean }): JSX.Element {
  const sessions = chatSessions.value;

  if (collapsed) {
    return <div style={{ flex: 1 }} aria-hidden="true" />;
  }

  if (sessions.length === 0) {
    return (
      <div
        style={{
          flex: 1,
          padding: 'var(--space-4)',
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-disabled)',
          textAlign: 'center',
        }}
      >
        还没有对话
      </div>
    );
  }

  // 按日期分组（今天/昨天/本周/更早）
  const groups = groupSessionsByDate(sessions);

  return (
    <nav
      aria-label="Sessions"
      style={{
        flex: 1,
        overflow: 'auto',
        padding: 'var(--space-2) 0',
      }}
    >
      {Object.entries(groups).map(([label, list]) =>
        list.length === 0 ? null : (
          <div key={label} style={{ marginBottom: 'var(--space-3)' }}>
            <div
              style={{
                padding: '0 var(--space-4)',
                fontSize: 'var(--text-xs)',
                color: 'var(--color-text-secondary)',
                fontWeight: 500,
                marginBottom: 'var(--space-1)',
              }}
            >
              {label}
            </div>
            {list.map((s) => (
              <SessionItem key={s.id} session={s} />
            ))}
          </div>
        ),
      )}
    </nav>
  );
}

function SessionItem({ session: s }: { session: { id: string; title: string } }): JSX.Element {
  const active = activeSessionId.value === s.id;
  return (
    <button
      type="button"
      onClick={() => {
        activeSessionId.value = s.id;
        currentView.value = 'chat';
      }}
      className="interactive"
      style={{
        display: 'block',
        width: '100%',
        padding: '6px var(--space-4)',
        background: active ? 'var(--color-surface-hover)' : 'transparent',
        border: 'none',
        borderLeft: active ? '2px solid var(--color-accent)' : '2px solid transparent',
        color: 'var(--color-text)',
        fontSize: 'var(--text-sm)',
        textAlign: 'left',
        cursor: 'pointer',
        whiteSpace: 'nowrap',
        overflow: 'hidden',
        textOverflow: 'ellipsis',
      }}
    >
      {s.title || '未命名对话'}
    </button>
  );
}

// ── ④ 次级导航 ──────────────────────────────────────────────
type NavItem = { view: View; icon: string; label: string };
const NAV_ITEMS: NavItem[] = [
  { view: 'items', icon: '📄', label: '条目' },
  { view: 'projects', icon: '🗂', label: 'Projects' },
  { view: 'remote', icon: '🔗', label: '远程目录' },
  { view: 'knowledge', icon: '📊', label: '知识全景' },
  { view: 'settings', icon: '⚙', label: '设置' },
];

function SecondaryNav({ collapsed }: { collapsed: boolean }): JSX.Element {
  return (
    <nav
      aria-label="Features"
      style={{
        borderTop: '1px solid var(--color-border)',
        padding: 'var(--space-2) 0',
        display: 'flex',
        flexDirection: 'column',
        gap: 2,
      }}
    >
      {NAV_ITEMS.map((item) => {
        const active = currentView.value === item.view;
        return (
          <button
            key={item.view}
            type="button"
            onClick={() => (currentView.value = item.view)}
            aria-current={active ? 'page' : undefined}
            aria-label={item.label}
            className="interactive"
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 'var(--space-3)',
              padding: collapsed
                ? 'var(--space-2) 0'
                : 'var(--space-2) var(--space-4)',
              background: active ? 'var(--color-surface-hover)' : 'transparent',
              borderLeft: active
                ? '2px solid var(--color-accent)'
                : '2px solid transparent',
              border: 'none',
              borderLeftWidth: 2,
              borderLeftStyle: 'solid',
              borderLeftColor: active ? 'var(--color-accent)' : 'transparent',
              color: active ? 'var(--color-text)' : 'var(--color-text-secondary)',
              fontSize: 'var(--text-sm)',
              cursor: 'pointer',
              textAlign: 'left',
              justifyContent: collapsed ? 'center' : 'flex-start',
            }}
          >
            <span aria-hidden="true" style={{ fontSize: 'var(--text-base)' }}>
              {item.icon}
            </span>
            {!collapsed && <span>{item.label}</span>}
          </button>
        );
      })}
    </nav>
  );
}

// ── ⑤ 状态栏（vault + 连接） ────────────────────────────────
function StatusBar({ collapsed }: { collapsed: boolean }): JSX.Element {
  const menuOpen = useSignal(false);
  const conn = connectionState.value;
  const vault = vaultState.value;

  const connLabel = conn === 'online' ? t('conn.online') : conn === 'reconnecting' ? t('conn.reconnecting') : t('conn.offline');

  return (
    <div
      style={{
        borderTop: '1px solid var(--color-border)',
        padding: 'var(--space-3) var(--space-4)',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-2)',
        fontSize: 'var(--text-xs)',
        color: 'var(--color-text-secondary)',
        position: 'relative',
      }}
    >
      {!collapsed && (
        <button
          type="button"
          onClick={() => (menuOpen.value = !menuOpen.value)}
          aria-label="Account menu"
          aria-expanded={menuOpen.value}
          className="interactive"
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 'var(--space-2)',
            padding: '4px 6px',
            background: 'transparent',
            border: 'none',
            borderRadius: 'var(--radius-sm)',
            color: 'var(--color-text-secondary)',
            fontSize: 'var(--text-xs)',
            cursor: 'pointer',
            width: '100%',
            textAlign: 'left',
          }}
        >
          <span
            aria-hidden="true"
            style={{
              width: 24,
              height: 24,
              borderRadius: '50%',
              background: 'var(--color-accent)',
              color: 'white',
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              fontSize: 'var(--text-xs)',
              fontWeight: 600,
              flexShrink: 0,
            }}
          >
            U
          </span>
          <span style={{ flex: 1 }}>
            {vault === 'unlocked' ? '已解锁' : '已锁定'}
          </span>
        </button>
      )}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 'var(--space-2)',
          justifyContent: collapsed ? 'center' : 'flex-start',
        }}
      >
        <span className={`status-dot ${conn}`} />
        {!collapsed && <span>{connLabel}</span>}
      </div>

      {menuOpen.value && !collapsed && (
        <AccountMenu onClose={() => (menuOpen.value = false)} />
      )}
    </div>
  );
}

function AccountMenu({ onClose }: { onClose: () => void }): JSX.Element {
  return (
    <div
      role="menu"
      className="fade-slide-in"
      style={{
        position: 'absolute',
        bottom: 'calc(100% - var(--space-2))',
        left: 'var(--space-3)',
        right: 'var(--space-3)',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        boxShadow: 'var(--shadow-lg)',
        padding: 'var(--space-1) 0',
        zIndex: 10,
      }}
    >
      <MenuItem onClick={() => { currentView.value = 'settings'; onClose(); }}>
        ⚙ 设置
      </MenuItem>
      <MenuItem onClick={() => { onClose(); }}>
        🔒 锁定 vault
      </MenuItem>
      <MenuItem onClick={() => { onClose(); }}>
        🌓 切换主题
      </MenuItem>
      <div style={{ height: 1, background: 'var(--color-border)', margin: 'var(--space-1) 0' }} />
      <MenuItem onClick={() => { onClose(); }}>
        关于 Attune
      </MenuItem>
    </div>
  );
}

function MenuItem({ onClick, children }: { onClick: () => void; children: JSX.Element | string }): JSX.Element {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      className="interactive"
      style={{
        display: 'block',
        width: '100%',
        padding: '6px var(--space-3)',
        background: 'transparent',
        border: 'none',
        color: 'var(--color-text)',
        fontSize: 'var(--text-sm)',
        textAlign: 'left',
        cursor: 'pointer',
      }}
      onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--color-surface-hover)')}
      onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
    >
      {children}
    </button>
  );
}

// ── 辅助 ────────────────────────────────────────────────────
function groupSessionsByDate<T extends { created_at: string }>(sessions: T[]): Record<string, T[]> {
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterdayStart = new Date(todayStart.getTime() - 86_400_000);
  const weekStart = new Date(todayStart.getTime() - 7 * 86_400_000);

  const groups: Record<string, T[]> = {
    今天: [],
    昨天: [],
    本周: [],
    更早: [],
  };

  for (const s of sessions) {
    const d = new Date(s.created_at);
    if (d >= todayStart) groups['今天']!.push(s);
    else if (d >= yesterdayStart) groups['昨天']!.push(s);
    else if (d >= weekStart) groups['本周']!.push(s);
    else groups['更早']!.push(s);
  }
  return groups;
}
