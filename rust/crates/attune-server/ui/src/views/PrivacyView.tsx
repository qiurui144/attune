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
import { useConfirm } from '../components/ConfirmModal';
import { Button } from '../components/Button';

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

// INT-2 doc-privacy scan result (POST /doc-privacy/scan) — privacy-first: never
// carries PII values, only a grade + summary counts.
interface DocScanResult {
  classification: 'normal' | 'sensitive_partial' | 'classified';
  blocked: boolean;
  block_reason: string | null;
  warning: string | null;
  pii_count: number;
}

type RedactMode = 'reversible' | 'irreversible';

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
  const { confirm, confirmModal } = useConfirm();

  // INT-2 doc-export panel interactive state.
  const scanText = useSignal('');
  const scanResult = useSignal<DocScanResult | null>(null);
  const scanBusy = useSignal(false);
  const redactMode = useSignal<RedactMode>('reversible');

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

  // Load the persisted doc-redaction mode (settings.privacy.doc_redact_mode).
  async function loadRedactMode(): Promise<void> {
    try {
      const settings = await api.get<{ privacy?: { doc_redact_mode?: string } }>('/settings');
      const m = settings.privacy?.doc_redact_mode;
      if (m === 'irreversible' || m === 'reversible') {
        redactMode.value = m;
      }
    } catch {
      /* settings unreadable (vault locked) → keep default reversible */
    }
  }

  useEffect(() => {
    void refresh();
    void loadRedactMode();
  }, []);

  // INT-2: scan arbitrary text for confidentiality grade + PII count (🆓, no DEK).
  async function runScan(): Promise<void> {
    const text = scanText.value.trim();
    if (text.length === 0) return;
    scanBusy.value = true;
    try {
      scanResult.value = await api.post<DocScanResult>('/doc-privacy/scan', { text });
    } catch {
      toast('error', t('privacy.docExport.scanFailed'));
    } finally {
      scanBusy.value = false;
    }
  }

  // INT-2: persist the redaction mode (reversible [KIND_N] vs irreversible mask).
  async function setRedactMode(mode: RedactMode): Promise<void> {
    redactMode.value = mode;
    try {
      await api.patch('/settings', { privacy: { doc_redact_mode: mode } });
    } catch {
      toast('error', t('privacy.errors.saveFailed'));
    }
  }

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
    if (!(await confirm({ title: t('confirm.title.lockVault'), message: t('privacy.confirm.lockNow'), danger: true }))) return;
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
    if (!(await confirm({ title: t('confirm.title.wipeCloud'), message: t('privacy.confirm.wipeCloud'), danger: true }))) return;
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
    if (!(await confirm({ title: t('confirm.title.deleteAccount'), message: t('privacy.confirm.deleteAccount'), danger: true }))) return;
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
            <Button variant="danger" size="sm" data-testid="vault-lock-now" onClick={() => void lockNow()}>
              {t('privacy.actions.lockNow')}
            </Button>
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
          <Button
            variant="secondary"
            size="sm"
            data-testid="wipe-cloud-session-button"
            onClick={() => void wipeCloud()}
            disabled={!s.outbound.cloud_saas.enabled}
          >
            {t('privacy.actions.wipeCloudSession')}
          </Button>
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

      {/* ── Document export protection (INT-2) ──── */}
      <Panel title={t('privacy.docExport.title')}>
        <p
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            margin: 0,
            marginBottom: 'var(--space-3)',
            lineHeight: 1.6,
          }}
        >
          {t('privacy.docExport.note')}
        </p>
        <div
          data-testid="doc-export-protection"
          style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}
        >
          <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text)' }}>
            {t('privacy.docExport.classifyActive')}
          </span>
          <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text)' }}>
            {t('privacy.docExport.confidentialBlock')}
          </span>
          <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text)' }}>
            {t('privacy.docExport.piiRedact')}
          </span>
        </div>
        <p
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            margin: 'var(--space-3) 0 0',
            lineHeight: 1.5,
          }}
        >
          {t('privacy.docExport.pdfHint')}
        </p>

        {/* ── Redaction mode (reversible / irreversible) ── */}
        <div style={{ marginTop: 'var(--space-4)', borderTop: '1px solid var(--color-border)', paddingTop: 'var(--space-3)' }}>
          <div style={{ fontSize: 'var(--text-sm)', fontWeight: 500, marginBottom: 'var(--space-2)' }}>
            {t('privacy.docExport.modeTitle')}
          </div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
            {(['reversible', 'irreversible'] as RedactMode[]).map((m) => (
              <label
                key={m}
                style={{ display: 'flex', alignItems: 'flex-start', gap: 'var(--space-2)', cursor: 'pointer', fontSize: 'var(--text-sm)' }}
              >
                <input
                  type="radio"
                  name="doc-redact-mode"
                  data-testid={`redact-mode-${m}`}
                  checked={redactMode.value === m}
                  onChange={() => void setRedactMode(m)}
                  style={{ marginTop: '3px' }}
                />
                <span>
                  <span style={{ color: 'var(--color-text)' }}>
                    {m === 'reversible' ? t('privacy.docExport.modeReversible') : t('privacy.docExport.modeIrreversible')}
                  </span>
                  <span style={{ display: 'block', fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', lineHeight: 1.5 }}>
                    {m === 'reversible' ? t('privacy.docExport.modeReversibleDesc') : t('privacy.docExport.modeIrreversibleDesc')}
                  </span>
                </span>
              </label>
            ))}
          </div>
        </div>

        {/* ── Scan / export-preview entry ── */}
        <div style={{ marginTop: 'var(--space-4)', borderTop: '1px solid var(--color-border)', paddingTop: 'var(--space-3)' }}>
          <div style={{ fontSize: 'var(--text-sm)', fontWeight: 500, marginBottom: 'var(--space-2)' }}>
            {t('privacy.docExport.scanTitle')}
          </div>
          <textarea
            data-testid="doc-scan-input"
            value={scanText.value}
            onInput={(e: Event) => {
              scanText.value = (e.currentTarget as HTMLTextAreaElement).value;
            }}
            placeholder={t('privacy.docExport.scanPlaceholder')}
            rows={3}
            style={{
              width: '100%',
              boxSizing: 'border-box',
              fontSize: 'var(--text-sm)',
              padding: 'var(--space-2)',
              border: '1px solid var(--color-border)',
              borderRadius: 'var(--radius-sm)',
              background: 'var(--color-bg)',
              color: 'var(--color-text)',
              resize: 'vertical',
            }}
          />
          <div style={{ marginTop: 'var(--space-2)' }}>
            <Button
              variant="secondary"
              size="sm"
              data-testid="doc-scan-button"
              onClick={() => void runScan()}
              disabled={scanBusy.value || scanText.value.trim().length === 0}
            >
              {scanBusy.value ? t('common.loading') : t('privacy.docExport.scanButton')}
            </Button>
          </div>
          {scanResult.value !== null && (
            <div
              data-testid="doc-scan-result"
              style={{
                marginTop: 'var(--space-3)',
                fontSize: 'var(--text-sm)',
                padding: 'var(--space-3)',
                borderRadius: 'var(--radius-sm)',
                border: '1px solid var(--color-border)',
                background: 'var(--color-bg)',
              }}
            >
              <div style={{ fontWeight: 500, color: scanResult.value.blocked ? 'var(--color-danger)' : 'var(--color-text)' }}>
                {scanResult.value.classification === 'classified'
                  ? t('privacy.docExport.gradeClassified')
                  : scanResult.value.classification === 'sensitive_partial'
                    ? t('privacy.docExport.gradeSensitive')
                    : t('privacy.docExport.gradeNormal')}
              </div>
              <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', marginTop: 'var(--space-1)' }}>
                {scanResult.value.blocked
                  ? t('privacy.docExport.scanBlocked')
                  : t('privacy.docExport.scanPiiCount', { n: scanResult.value.pii_count })}
              </div>
            </div>
          )}
        </div>
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
          <Button variant="primary" size="sm" data-testid="dsar-export-button" onClick={() => void exportData()}>
            {t('privacy.actions.exportData')}
          </Button>
          <Button variant="danger" size="sm" data-testid="dsar-delete-button" onClick={() => void deleteAccount()}>
            {t('privacy.actions.deleteAccount')}
          </Button>
        </div>
      </Panel>
      {confirmModal}
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

