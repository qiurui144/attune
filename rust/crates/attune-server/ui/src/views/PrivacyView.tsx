/**
 * PrivacyView · v1.0.6 Privacy Logic SSOT dashboard
 *
 * 见 spec `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md` §3-5
 *
 * 5 出网点 (LLM / Cloud SaaS / WebDAV / Web Search / Telemetry) 单一总览，
 * 全部默认关闭。用户在此页面可：
 *   - 查看 vault 锁定状态 + 立即锁定
 *   - 切换 5 出网点开关
 *   - 一键清除 cloud session
 *   - 进入 DSAR 导出 / 删除 / 审计日志
 *
 * 后端 API: `routes/privacy.rs` (status / settings PATCH / lock / wipe-cloud-session)
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { api, ApiError } from '../store/api';
import { vaultState } from '../store/signals';
import { t } from '../i18n';
import { toast } from '../components/Toast';

// 与后端 `routes/privacy.rs::PRIVACY_KEYS` 严格对齐 (5 个 outbound + tour 标记)
type OutboundKey = 'llm' | 'cloud_saas' | 'webdav' | 'web_search' | 'telemetry';
const OUTBOUND_KEYS: OutboundKey[] = ['llm', 'cloud_saas', 'webdav', 'web_search', 'telemetry'];

interface OutboundEntry {
  enabled: boolean;
}

interface PrivacyStatus {
  outbound: Record<OutboundKey, OutboundEntry>;
  vault: { state: 'sealed' | 'locked' | 'unlocked' };
  redactor: { patterns_active: number; l1_active: boolean };
  privacy_tour_seen?: boolean;
}

const LABEL_KEY_FOR: Record<OutboundKey, string> = {
  llm: 'privacy.outbound.llm',
  cloud_saas: 'privacy.outbound.cloudSaas',
  webdav: 'privacy.outbound.webdav',
  web_search: 'privacy.outbound.webSearch',
  telemetry: 'privacy.outbound.telemetry',
};

const DESC_KEY_FOR: Record<OutboundKey, string> = {
  llm: 'privacy.outbound.llmDesc',
  cloud_saas: 'privacy.outbound.cloudSaasDesc',
  webdav: 'privacy.outbound.webdavDesc',
  web_search: 'privacy.outbound.webSearchDesc',
  telemetry: 'privacy.outbound.telemetryDesc',
};

export function PrivacyView(): JSX.Element {
  const status = useSignal<PrivacyStatus | null>(null);
  const busyKey = useSignal<OutboundKey | null>(null);

  async function refresh(): Promise<void> {
    try {
      const data = await api.get<PrivacyStatus>('/privacy/status');
      status.value = data;
    } catch (err) {
      if (!(err instanceof ApiError && err.status === 401)) {
        toast('error', t('privacy.errors.loadFailed'));
      }
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function toggle(key: OutboundKey, next: boolean): Promise<void> {
    busyKey.value = key;
    try {
      await api.patch('/privacy/settings', { [key]: next });
      await refresh();
    } catch {
      toast('error', t('privacy.errors.saveFailed'));
    } finally {
      busyKey.value = null;
    }
  }

  async function lockNow(): Promise<void> {
    if (!confirm(t('privacy.confirm.lockNow'))) return;
    try {
      await api.post('/privacy/lock');
      vaultState.value = 'locked';
      await refresh();
      toast('success', t('privacy.success.locked'));
    } catch {
      toast('error', t('privacy.errors.lockFailed'));
    }
  }

  async function wipeCloud(): Promise<void> {
    if (!confirm(t('privacy.confirm.wipeCloud'))) return;
    try {
      await api.post('/privacy/wipe-cloud-session');
      await refresh();
      toast('success', t('privacy.success.cloudWiped'));
    } catch {
      toast('error', t('privacy.errors.wipeFailed'));
    }
  }

  async function exportData(): Promise<void> {
    try {
      await api.post('/dsar/export');
      toast('success', t('privacy.success.dsarRequested'));
    } catch {
      toast('error', t('privacy.errors.dsarFailed'));
    }
  }

  async function deleteAccount(): Promise<void> {
    if (!confirm(t('privacy.confirm.deleteAccount'))) return;
    try {
      await api.post('/dsar/delete');
      toast('success', t('privacy.success.deleteRequested'));
    } catch {
      toast('error', t('privacy.errors.deleteFailed'));
    }
  }

  if (status.value === null) {
    return (
      <div
        data-testid="privacy-view"
        style={{
          padding: 'var(--space-5)',
          color: 'var(--color-text-secondary)',
          fontSize: 'var(--text-sm)',
        }}
      >
        {t('common.loading')}
      </div>
    );
  }

  const s = status.value;

  return (
    <div
      data-testid="privacy-view"
      style={{
        maxWidth: 880,
        margin: '0 auto',
        padding: 'var(--space-5)',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-5)',
      }}
    >
      <header>
        <h1
          style={{
            fontSize: 'var(--text-2xl)',
            fontWeight: 600,
            margin: 0,
            marginBottom: 'var(--space-2)',
          }}
        >
          {t('privacy.title')}
        </h1>
        <p
          style={{
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text-secondary)',
            margin: 0,
            lineHeight: 1.6,
          }}
        >
          {t('privacy.subtitle')}
        </p>
      </header>

      {/* ── Vault state ────────────────────────── */}
      <Panel title={t('privacy.vault.state')}>
        <div
          style={{
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
            gap: 'var(--space-3)',
          }}
        >
          <span
            data-testid="vault-state"
            style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text)' }}
          >
            {s.vault.state === 'unlocked'
              ? t('privacy.vault.unlocked')
              : s.vault.state === 'locked'
                ? t('privacy.vault.locked')
                : t('privacy.vault.sealed')}
          </span>
          {s.vault.state === 'unlocked' && (
            <button
              type="button"
              data-testid="vault-lock-now"
              onClick={() => void lockNow()}
              className="interactive"
              style={primaryButton}
            >
              {t('privacy.actions.lockNow')}
            </button>
          )}
        </div>
      </Panel>

      {/* ── 5 outbound toggles ──────────────────── */}
      <Panel title={t('privacy.outbound.title')}>
        <p
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            margin: 0,
            marginBottom: 'var(--space-3)',
            lineHeight: 1.6,
          }}
        >
          {t('privacy.outbound.note')}
        </p>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
          {OUTBOUND_KEYS.map((k) => {
            const entry = s.outbound[k];
            const enabled = entry?.enabled ?? false;
            const busy = busyKey.value === k;
            return (
              <div
                key={k}
                data-testid={`outbound-row-${k}`}
                style={{
                  display: 'flex',
                  alignItems: 'flex-start',
                  justifyContent: 'space-between',
                  gap: 'var(--space-3)',
                  padding: 'var(--space-3) 0',
                  borderTop: '1px solid var(--color-border)',
                }}
              >
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 'var(--text-sm)', fontWeight: 500 }}>
                    {t(LABEL_KEY_FOR[k])}
                  </div>
                  <div
                    style={{
                      fontSize: 'var(--text-xs)',
                      color: 'var(--color-text-secondary)',
                      marginTop: 'var(--space-1)',
                      lineHeight: 1.5,
                    }}
                  >
                    {t(DESC_KEY_FOR[k])}
                  </div>
                </div>
                <label
                  style={{
                    display: 'inline-flex',
                    alignItems: 'center',
                    gap: 'var(--space-2)',
                    cursor: busy ? 'wait' : 'pointer',
                    fontSize: 'var(--text-sm)',
                  }}
                >
                  <input
                    type="checkbox"
                    data-testid={`toggle-${k}`}
                    checked={enabled}
                    disabled={busy}
                    onChange={(e: Event) => {
                      const target = e.currentTarget as HTMLInputElement;
                      void toggle(k, target.checked);
                    }}
                  />
                  <span style={{ color: 'var(--color-text-secondary)' }}>
                    {enabled ? t('privacy.outbound.enabled') : t('privacy.outbound.disabled')}
                  </span>
                </label>
              </div>
            );
          })}
        </div>

        <div style={{ marginTop: 'var(--space-4)' }}>
          <button
            type="button"
            data-testid="wipe-cloud-session-button"
            onClick={() => void wipeCloud()}
            className="interactive"
            style={secondaryButton}
            disabled={!s.outbound.cloud_saas.enabled}
          >
            {t('privacy.actions.wipeCloudSession')}
          </button>
          <p
            style={{
              fontSize: 'var(--text-xs)',
              color: 'var(--color-text-secondary)',
              margin: 'var(--space-2) 0 0',
              lineHeight: 1.5,
            }}
          >
            {t('privacy.actions.wipeCloudSessionHint')}
          </p>
        </div>
      </Panel>

      {/* ── PII Redactor ──────────────────────── */}
      <Panel title={t('privacy.redactor.title')}>
        <p
          style={{
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text)',
            margin: 0,
            lineHeight: 1.6,
          }}
        >
          {t('privacy.redactor.patternsActive', { n: s.redactor.patterns_active })}
        </p>
        <p
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            margin: 'var(--space-2) 0 0',
            lineHeight: 1.5,
          }}
        >
          {s.redactor.l1_active ? t('privacy.redactor.l1Active') : t('privacy.redactor.l1Missing')}
        </p>
      </Panel>

      {/* ── DSAR + Audit ───────────────────────── */}
      <Panel title={t('privacy.dsar.title')}>
        <p
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            margin: 0,
            marginBottom: 'var(--space-3)',
            lineHeight: 1.6,
          }}
        >
          {t('privacy.dsar.note')}
        </p>
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 'var(--space-2)' }}>
          <button
            type="button"
            data-testid="dsar-export-button"
            onClick={() => void exportData()}
            className="interactive"
            style={primaryButton}
          >
            {t('privacy.actions.exportData')}
          </button>
          <button
            type="button"
            data-testid="dsar-delete-button"
            onClick={() => void deleteAccount()}
            className="interactive"
            style={dangerButton}
          >
            {t('privacy.actions.deleteAccount')}
          </button>
        </div>
      </Panel>
    </div>
  );
}

// ── Local layout helpers (self-contained; SettingsView's Section is private) ─

function Panel({
  title,
  children,
}: {
  title: string;
  children: preact.ComponentChildren;
}): JSX.Element {
  return (
    <section
      style={{
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        padding: 'var(--space-4)',
      }}
    >
      <h2
        style={{
          fontSize: 'var(--text-sm)',
          fontWeight: 600,
          margin: 0,
          marginBottom: 'var(--space-3)',
          color: 'var(--color-text-secondary)',
          textTransform: 'uppercase',
          letterSpacing: '0.05em',
        }}
      >
        {title}
      </h2>
      {children}
    </section>
  );
}

const primaryButton: JSX.CSSProperties = {
  padding: 'var(--space-2) var(--space-4)',
  background: 'var(--color-accent)',
  color: 'white',
  border: 'none',
  borderRadius: 'var(--radius-md)',
  fontSize: 'var(--text-sm)',
  cursor: 'pointer',
};

const secondaryButton: JSX.CSSProperties = {
  padding: 'var(--space-2) var(--space-4)',
  background: 'transparent',
  color: 'var(--color-text)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  fontSize: 'var(--text-sm)',
  cursor: 'pointer',
};

const dangerButton: JSX.CSSProperties = {
  padding: 'var(--space-2) var(--space-4)',
  background: 'transparent',
  color: 'var(--color-danger, #b91c1c)',
  border: '1px solid var(--color-danger, #b91c1c)',
  borderRadius: 'var(--radius-md)',
  fontSize: 'var(--text-sm)',
  cursor: 'pointer',
};
