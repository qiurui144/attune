/** Quota dashboard 视图 — v1.0.7 client 端
 *
 * 调 cloud accounts /api/v1/users/me/quota 显示:
 *   - 当前 tier + plan_expires
 *   - 本月 LLM token 用量 (input/output/total/cost)
 *   - quota 余额 + percent_used progress bar
 *   - history (近 3 个月 cost)
 *   - cross_service_errors (跨服务不可达提示)
 *
 * 升级 CTA: free tier 用满 (percent_used > 80) 时显示 upgrade button →
 * 打开 SettingsView member tab (per visual unification spec).
 *
 * Per CLAUDE.md § Cost & Trigger Contract: 用户必须能"一眼看见花了多少 token、
 * 还剩多少 quota、下次 renew 时间"。
 *
 * Style: ALL inline (OfficeView pattern); design tokens only.
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState, Skeleton } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import { api } from '../store/api';
import { currentView, settingsInitialTab } from '../store/signals';
import { openMemberBilling } from '../hooks/useMember';

function friendlyServiceError(raw: string): string {
  const r = raw.toUpperCase();
  if (r.includes('EHOSTUNREACH') || r.includes('ENETUNREACH') || r.includes('ENOTFOUND')) {
    return t('quota.error.network_unreachable');
  }
  if (r.includes('ECONNREFUSED') || r.includes('503') || r.includes('502')) {
    return t('quota.error.service_offline');
  }
  if (r.includes('401') || r.includes('403') || r.includes('UNAUTHORIZED')) {
    return t('quota.error.auth_failed');
  }
  if (r.includes('TIMEOUT') || r.includes('ETIMEDOUT')) {
    return t('quota.error.timeout');
  }
  return t('quota.error.generic');
}

interface QuotaUsage {
  events?: number;
  llm_tokens_input: number;
  llm_tokens_output: number;
  llm_tokens_total: number;
  llm_cost_usd: number;
  plugin_installs: number;
  cache_hit_rate?: number;
  prompt_cache_hit_rate?: number;
}

interface QuotaLimits {
  llm_tokens_monthly: number;
  remaining: number;
  percent_used: number;
}

interface QuotaHistoryEntry {
  month: string;
  llm_tokens_total: number;
  llm_cost_usd: number;
}

interface QuotaResponse {
  tier: string;
  plan_expires: string | null;
  month: string;
  usage: QuotaUsage;
  local_usage?: QuotaUsage;
  quota: QuotaLimits;
  history: QuotaHistoryEntry[];
  cross_service_errors: Record<string, string>;
}

const UPGRADE_THRESHOLD_PERCENT = 80;

function formatNumber(n: number): string {
  return n.toLocaleString('en-US');
}

function formatCost(usd: number): string {
  return `$${usd.toFixed(4)}`;
}

function progressColor(percent: number): string {
  if (percent >= 90) return 'var(--color-danger, #ef4444)';
  if (percent >= 70) return 'var(--color-warning, #f59e0b)';
  return 'var(--color-accent, #3b82f6)';
}

// ── shared inline styles ──
const containerStyle: JSX.CSSProperties = {
  padding: 'var(--space-6)',
  maxWidth: 880,
  margin: '0 auto',
};

const sectionStyle: JSX.CSSProperties = {
  marginBottom: 'var(--space-5)',
};

const sectionTitleStyle: JSX.CSSProperties = {
  fontSize: 'var(--text-base)',
  fontWeight: 600,
  margin: '0 0 var(--space-3) 0',
};

const statCardStyle: JSX.CSSProperties = {
  padding: 'var(--space-3)',
  background: 'var(--color-surface)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
};

const warnBannerStyle: JSX.CSSProperties = {
  padding: 'var(--space-3)',
  background: 'var(--color-warning-bg, #fef3c7)',
  border: '1px solid var(--color-warning, #f59e0b)',
  borderRadius: 'var(--radius-md)',
  marginBottom: 'var(--space-4)',
  fontSize: 'var(--text-sm)',
};

const infoBannerStyle: JSX.CSSProperties = {
  padding: 'var(--space-3)',
  background: 'var(--color-surface)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  marginBottom: 'var(--space-4)',
  fontSize: 'var(--text-sm)',
  color: 'var(--color-text-secondary)',
};

const tableStyle: JSX.CSSProperties = {
  width: '100%',
  borderCollapse: 'collapse',
  fontSize: 'var(--text-sm)',
};

const thStyle: JSX.CSSProperties = {
  textAlign: 'left' as const,
  padding: 'var(--space-2) var(--space-1)',
  borderBottom: '1px solid var(--color-border)',
  fontWeight: 600,
};

const tdStyle: JSX.CSSProperties = {
  padding: 'var(--space-2) var(--space-1)',
  borderBottom: '1px solid var(--color-border)',
};

export function QuotaView(): JSX.Element {
  const data = useSignal<QuotaResponse | null>(null);
  const loading = useSignal(true);
  const error = useSignal<string | null>(null);
  const openingUpgrade = useSignal(false);

  useEffect(() => {
    void refresh();
  }, []);

  async function refresh(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      const resp = await api.get<QuotaResponse>('/users/me/quota');
      data.value = resp;
    } catch (e) {
      error.value = (e as Error).message || 'unknown';
      toast('error', t('quota.load_failed'));
    } finally {
      loading.value = false;
    }
  }

  async function openUpgrade(): Promise<void> {
    if (openingUpgrade.value) return;
    openingUpgrade.value = true;
    try {
      await openMemberBilling('upgrade');
    } catch {
      toast('error', t('quota.upgrade_open_failed'));
      settingsInitialTab.value = 'member';
      currentView.value = 'settings';
    } finally {
      openingUpgrade.value = false;
    }
  }

  if (loading.value && !data.value) {
    return (
      <div style={containerStyle}>
        <Skeleton width="100%" height={120} />
        <div style={{ marginTop: 'var(--space-4)' }}>
          <Skeleton width="100%" height={60} />
        </div>
      </div>
    );
  }

  if (error.value && !data.value) {
    return (
      <EmptyState icon="⚠" title={t('quota.error_title')} description={error.value}
        actions={[{ label: t('quota.retry'), onClick: refresh }]} />
    );
  }

  if (!data.value) {
    return <EmptyState icon="📊" title={t('quota.empty_title')} description={t('quota.empty_desc')} />;
  }

  const d = data.value;
  const isPaidTier = ['pro', 'pro_plus', 'enterprise', 'paid'].includes(d.tier);
  const showUpgrade = !isPaidTier || d.quota.percent_used >= UPGRADE_THRESHOLD_PERCENT;
  const upgradePrompt = isPaidTier ? t('quota.upgrade_prompt') : t('quota.upgrade_prompt_free');
  const hasErrors = Object.keys(d.cross_service_errors).length > 0;
  const showLocalUsage =
    Boolean(d.local_usage) && d.local_usage!.llm_tokens_total !== d.usage.llm_tokens_total;

  return (
    <div style={containerStyle}>
      {/* Header */}
      <header style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 'var(--space-5)' }}>
        <div>
          <h2 style={{ fontSize: 'var(--text-2xl)', fontWeight: 600, margin: 0 }}>
            {t('quota.title')}
          </h2>
          <p style={{ color: 'var(--color-text-secondary)', marginTop: 'var(--space-1)', fontSize: 'var(--text-sm)' }}>
            {t('quota.month_label')}: <strong>{d.month}</strong> · {t('quota.tier_label')}:{' '}
            <strong>{d.tier}</strong>
            {d.plan_expires && ` · ${t('quota.expires_label')}: ${d.plan_expires.slice(0, 10)}`}
          </p>
        </div>
        <Button variant="secondary" onClick={refresh}>{t('quota.refresh')}</Button>
      </header>

      {!isPaidTier && (
        <div style={infoBannerStyle}>
          {t('quota.self_managed_note')}
        </div>
      )}
      {isPaidTier && hasErrors && (
        <div style={infoBannerStyle}>
          {t('quota.member_unavailable_note')}
        </div>
      )}

      {/* Cross-service errors banner */}
      {hasErrors && (
        <div style={warnBannerStyle}>
          <strong>{t('quota.partial_data')}</strong>
          <ul style={{ margin: 'var(--space-1) 0 0', paddingLeft: 'var(--space-5)' }}>
            {Object.entries(d.cross_service_errors).map(([svc, msg]) => (
              <li key={svc} title={`${svc}: ${msg}`}>
                <code>{svc}</code>: {friendlyServiceError(msg)}
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* Usage section */}
      <section style={sectionStyle}>
        <h3 style={sectionTitleStyle}>{t('quota.usage_title')}</h3>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))', gap: 'var(--space-3)' }}>
          <StatCard label={t('quota.tokens_input')} value={formatNumber(d.usage.llm_tokens_input)} />
          <StatCard label={t('quota.tokens_output')} value={formatNumber(d.usage.llm_tokens_output)} />
          <StatCard label={t('quota.tokens_total')} value={formatNumber(d.usage.llm_tokens_total)} />
          <StatCard label={t('quota.cost')} value={formatCost(d.usage.llm_cost_usd)} />
          <StatCard label={t('quota.plugin_installs')} value={formatNumber(d.usage.plugin_installs)} />
        </div>
      </section>

      {showLocalUsage && d.local_usage && (
        <section style={sectionStyle}>
          <h3 style={sectionTitleStyle}>{t('quota.local_usage_title')}</h3>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))', gap: 'var(--space-3)' }}>
            <StatCard label={t('quota.tokens_input')} value={formatNumber(d.local_usage.llm_tokens_input)} />
            <StatCard label={t('quota.tokens_output')} value={formatNumber(d.local_usage.llm_tokens_output)} />
            <StatCard label={t('quota.tokens_total')} value={formatNumber(d.local_usage.llm_tokens_total)} />
            <StatCard label={t('quota.cost')} value={formatCost(d.local_usage.llm_cost_usd)} />
            <StatCard label={t('quota.local_events')} value={formatNumber(d.local_usage.events ?? 0)} />
          </div>
        </section>
      )}

      {/* Quota progress */}
      <section style={sectionStyle}>
        <h3 style={sectionTitleStyle}>{t('quota.budget_title')}</h3>
        <div style={{ ...statCardStyle }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 'var(--space-2)' }}>
            <span style={{ fontSize: 'var(--text-sm)' }}>
              {t('quota.used')}: <strong>{formatNumber(d.usage.llm_tokens_total)}</strong> /{' '}
              {formatNumber(d.quota.llm_tokens_monthly)}
            </span>
            <span style={{ color: progressColor(d.quota.percent_used), fontSize: 'var(--text-sm)' }}>
              <strong>{d.quota.percent_used.toFixed(1)}%</strong>
            </span>
          </div>
          <div style={{ height: 10, background: 'var(--color-border)', borderRadius: 5, overflow: 'hidden' }}>
            <div style={{
              height: '100%',
              width: `${Math.min(100, d.quota.percent_used)}%`,
              background: progressColor(d.quota.percent_used),
              transition: 'width var(--duration-base)',
            }} />
          </div>
          <p style={{ marginTop: 'var(--space-2)', fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>
            {t('quota.remaining')}: <strong>{formatNumber(d.quota.remaining)}</strong>{' '}
            {t('quota.tokens_unit')}
          </p>

          {showUpgrade && (
            <div style={{ marginTop: 'var(--space-4)', padding: 'var(--space-3)', background: 'rgba(212, 165, 116, 0.12)', borderRadius: 'var(--radius-md)' }}>
              <p style={{ margin: '0 0 var(--space-2) 0', fontSize: 'var(--text-sm)' }}>
                <strong>{upgradePrompt}</strong>
              </p>
              <Button variant="primary" loading={openingUpgrade.value} onClick={() => void openUpgrade()}>{t('quota.upgrade')}</Button>
            </div>
          )}
        </div>
      </section>

      {/* History */}
      <section>
        <h3 style={sectionTitleStyle}>{t('quota.history_title')}</h3>
        {d.history.length === 0 ? (
          <p style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-sm)' }}>
            {t('quota.history_empty')}
          </p>
        ) : (
          <table style={tableStyle}>
            <thead>
              <tr>
                <th style={thStyle}>{t('quota.col_month')}</th>
                <th style={{ ...thStyle, textAlign: 'right' }}>{t('quota.col_tokens')}</th>
                <th style={{ ...thStyle, textAlign: 'right' }}>{t('quota.col_cost')}</th>
              </tr>
            </thead>
            <tbody>
              {d.history.map((row) => (
                <tr key={row.month}>
                  <td style={tdStyle}>{row.month}</td>
                  <td style={{ ...tdStyle, textAlign: 'right' }}>{formatNumber(row.llm_tokens_total)}</td>
                  <td style={{ ...tdStyle, textAlign: 'right' }}>{formatCost(row.llm_cost_usd)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </div>
  );
}

interface StatCardProps {
  label: string;
  value: string;
}

function StatCard({ label, value }: StatCardProps): JSX.Element {
  return (
    <div style={statCardStyle}>
      <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>{label}</div>
      <div style={{ fontSize: 'var(--text-xl)', fontWeight: 600, marginTop: 'var(--space-1)' }}>{value}</div>
    </div>
  );
}
