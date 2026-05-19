/** Attune Sidebar · 左栏 5 区 · 可折叠
 * 见 spec §4 "Sidebar（左栏 · 5 区）"
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import {
  currentView,
  sidebarCollapsed,
  sidebarMoreExpanded,
  connectionState,
  chatSessions,
  activeSessionId,
  vaultState,
  theme,
} from '../store/signals';
import type { View } from '../store/signals';
import { loadSessions, clearActiveSession } from '../hooks/useChat';
import { t } from '../i18n';
import { api, clearToken } from '../store/api';
import { toast } from '../components/Toast';

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
          aria-label={collapsed ? t('sidebar.action.expand') : t('sidebar.action.collapse')}
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
          aria-label={t('sidebar.action.search')}
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
          <span style={{ flex: 1 }}>{t('shortcut.search')}…</span>
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
        aria-label={t('sidebar.action.new_chat')}
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
        {!collapsed && <span>{t('sidebar.new_chat')}</span>}
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
        {t('sidebar.no_sessions')}
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
      {Object.entries(groups).map(([groupKey, list]) =>
        list.length === 0 ? null : (
          <div key={groupKey} style={{ marginBottom: 'var(--space-3)' }}>
            <div
              style={{
                padding: '0 var(--space-4)',
                fontSize: 'var(--text-xs)',
                color: 'var(--color-text-secondary)',
                fontWeight: 500,
                marginBottom: 'var(--space-1)',
              }}
            >
              {t(`sidebar.session.${groupKey}`)}
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
      {s.title || t('sidebar.untitled_session')}
    </button>
  );
}

// ── ④ 次级导航 ──────────────────────────────────────────────
type NavItem = { view: View; icon: string; labelKey: string };

// Primary tier — always visible
const PRIMARY_NAV: NavItem[] = [
  { view: 'items', icon: '📄', labelKey: 'sidebar.nav.items' },
  { view: 'projects', icon: '🗂', labelKey: 'sidebar.nav.projects' },
  { view: 'knowledge', icon: '📊', labelKey: 'sidebar.nav.knowledge' },
];

// Secondary tier — inside collapsible "更多" group
const MORE_NAV: NavItem[] = [
  { view: 'remote', icon: '🔗', labelKey: 'sidebar.nav.remote' },
  { view: 'skills', icon: '🧠', labelKey: 'sidebar.nav.skills' },
  { view: 'marketplace', icon: '🏪', labelKey: 'sidebar.nav.marketplace' },
];

const MORE_VIEWS = new Set<View>(MORE_NAV.map((i) => i.view));

function NavButton({ item, collapsed }: { item: NavItem; collapsed: boolean }): JSX.Element {
  const active = currentView.value === item.view;
  return (
    <button
      type="button"
      onClick={() => (currentView.value = item.view)}
      aria-current={active ? 'page' : undefined}
      aria-label={t(item.labelKey)}
      className="interactive"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-3)',
        padding: collapsed
          ? 'var(--space-2) 0'
          : 'var(--space-2) var(--space-4)',
        background: active ? 'var(--color-surface-hover)' : 'transparent',
        border: 'none',
        borderLeftWidth: 2,
        borderLeftStyle: 'solid',
        borderLeftColor: active ? 'var(--color-accent)' : 'transparent',
        color: active ? 'var(--color-text)' : 'var(--color-text-secondary)',
        fontSize: 'var(--text-sm)',
        cursor: 'pointer',
        textAlign: 'left',
        justifyContent: collapsed ? 'center' : 'flex-start',
        width: '100%',
      }}
    >
      <span aria-hidden="true" style={{ fontSize: 'var(--text-base)' }}>
        {item.icon}
      </span>
      {!collapsed && <span>{t(item.labelKey)}</span>}
    </button>
  );
}

function SecondaryNav({ collapsed }: { collapsed: boolean }): JSX.Element {
  const activeInMore = MORE_VIEWS.has(currentView.value);
  // Auto-expand when active view is inside the "more" group (effect, not render mutation)
  useEffect(() => {
    if (activeInMore && !sidebarMoreExpanded.value) {
      sidebarMoreExpanded.value = true;
    }
  }, [activeInMore]);
  const moreExpanded = sidebarMoreExpanded.value;

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
      {/* Primary items — always visible */}
      {PRIMARY_NAV.map((item) => (
        <NavButton key={item.view} item={item} collapsed={collapsed} />
      ))}

      {/* "更多" group — in collapsed (icon) mode show all MORE items flat, no toggle */}
      {collapsed ? (
        MORE_NAV.map((item) => (
          <NavButton key={item.view} item={item} collapsed={collapsed} />
        ))
      ) : (
        <>
          {/* Toggle row */}
          <button
            type="button"
            aria-expanded={moreExpanded}
            aria-label={moreExpanded ? t('sidebar.nav.more.collapse_aria') : t('sidebar.nav.more.aria')}
            onClick={() => (sidebarMoreExpanded.value = !moreExpanded)}
            className="interactive"
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 'var(--space-3)',
              padding: 'var(--space-2) var(--space-4)',
              background: 'transparent',
              border: 'none',
              borderLeftWidth: 2,
              borderLeftStyle: 'solid',
              borderLeftColor: 'transparent',
              color: 'var(--color-text-secondary)',
              fontSize: 'var(--text-sm)',
              cursor: 'pointer',
              textAlign: 'left',
              width: '100%',
            }}
          >
            <span aria-hidden="true" style={{ fontSize: 'var(--text-xs)', lineHeight: 1 }}>
              {moreExpanded ? '▾' : '▸'}
            </span>
            <span style={{ flex: 1 }}>{t('sidebar.nav.more')}</span>
          </button>

          {/* Collapsible items */}
          {moreExpanded &&
            MORE_NAV.map((item) => (
              <NavButton key={item.view} item={item} collapsed={false} />
            ))}
        </>
      )}
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
          aria-label={t('sidebar.accountMenu')}
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
            {vault === 'unlocked' ? t('sidebar.vault.unlocked') : t('sidebar.vault.locked')}
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
        {t('sidebar.menu.settings')}
      </MenuItem>
      <MenuItem onClick={async () => {
        // UI-S8 fix (2026-05-02): 之前仅关闭菜单，**未实际锁定 vault**。
        // 现在走与 SettingsView 同一路径：调 /vault/lock + 清 token + reload。
        onClose();
        if (!confirm(t('sidebar.menu.lock_vault.confirm'))) return;
        try {
          await api.post('/vault/lock');
          clearToken();
          location.reload();
        } catch (e) {
          toast('error', `${t('sidebar.menu.lock_vault.error')}：${e instanceof Error ? e.message : String(e)}`);
        }
      }}>
        {t('sidebar.menu.lock_vault')}
      </MenuItem>
      <MenuItem onClick={() => {
        // 在 light → dark → auto 之间循环
        const next = theme.value === 'light' ? 'dark' : theme.value === 'dark' ? 'auto' : 'light';
        theme.value = next;
        onClose();
      }}>
        {t('sidebar.menu.toggle_theme')}
      </MenuItem>
      <div style={{ height: 1, background: 'var(--color-border)', margin: 'var(--space-1) 0' }} />
      <MenuItem onClick={() => {
        toast('info', t('sidebar.menu.about.toast'));
        onClose();
      }}>
        {t('sidebar.menu.about')}
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

  // i18n: 用稳定 key 作为 group id，UI 渲染时再 t() 翻译
  const groups: Record<string, T[]> = {
    today: [],
    yesterday: [],
    this_week: [],
    older: [],
  };

  for (const s of sessions) {
    const d = new Date(s.created_at);
    if (d >= todayStart) groups['today']!.push(s);
    else if (d >= yesterdayStart) groups['yesterday']!.push(s);
    else if (d >= weekStart) groups['this_week']!.push(s);
    else groups['older']!.push(s);
  }
  return groups;
}
