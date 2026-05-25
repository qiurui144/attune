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
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState, Skeleton } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import { api } from '../store/api';

interface QuotaUsage {
  llm_tokens_input: number;
  llm_tokens_output: number;
  llm_tokens_total: number;
  llm_cost_usd: number;
  plugin_installs: number;
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

export function QuotaView(): JSX.Element {
  const data = useSignal<QuotaResponse | null>(null);
  const loading = useSignal(true);
  const error = useSignal<string | null>(null);

  useEffect(() => {
    void refresh();
  }, []);

  async function refresh(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      // /api/v1/users/me/quota — 走 cloud accounts (gateway 透传).
      // 如果 attune-server 没 wire 到 cloud, 退到本地 stub.
      const resp = await api.get<QuotaResponse>('/users/me/quota');
      data.value = resp;
    } catch (e) {
      error.value = (e as Error).message || 'unknown';
      toast('error', t('quota.load_failed'));
    } finally {
      loading.value = false;
    }
  }

  function openUpgrade(): void {
    // 跳转 SettingsView member tab — pricing CTA (per memberLogin flow).
    // 临时方案:开浏览器到 cloud 定价页;后续可改成 in-app modal.
    window.open('https://attune.engi-stack.com/pricing', '_blank');
  }

  if (loading.value && !data.value) {
    return (
      <div style={{ padding: 24 }}>
        <Skeleton width="100%" height={120} />
        <div style={{ marginTop: 16 }}>
          <Skeleton width="100%" height={60} />
        </div>
      </div>
    );
  }

  if (error.value && !data.value) {
    return (
      <EmptyState
        icon="⚠"
        title={t('quota.error_title')}
        description={error.value}
        actions={[{ label: t('quota.retry'), onClick: refresh }]}
      />
    );
  }

  if (!data.value) {
    return <EmptyState icon="📊" title={t('quota.empty_title')} description={t('quota.empty_desc')} />;
  }

  const d = data.value;
  const showUpgrade = d.tier === 'individual' && d.quota.percent_used >= UPGRADE_THRESHOLD_PERCENT;
  const hasErrors = Object.keys(d.cross_service_errors).length > 0;

  return (
    <div style={{ padding: 24, maxWidth: 880 }}>
      {/* Header */}
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 24 }}>
        <div>
          <h2 style={{ margin: 0 }}>{t('quota.title')}</h2>
          <p style={{ color: 'var(--color-text-secondary)', marginTop: 4 }}>
            {t('quota.month_label')}: <strong>{d.month}</strong> · {t('quota.tier_label')}:{' '}
            <strong>{d.tier}</strong>
            {d.plan_expires && ` · ${t('quota.expires_label')}: ${d.plan_expires.slice(0, 10)}`}
          </p>
        </div>
        <Button variant="secondary" onClick={refresh}>
          {t('quota.refresh')}
        </Button>
      </div>

      {/* Cross-service errors banner */}
      {hasErrors && (
        <div
          style={{
            padding: 12,
            background: 'var(--color-warning-bg, #fef3c7)',
            border: '1px solid var(--color-warning, #f59e0b)',
            borderRadius: 6,
            marginBottom: 16,
            fontSize: 13,
          }}
        >
          <strong>{t('quota.partial_data')}</strong>
          <ul style={{ margin: '4px 0 0 0', paddingLeft: 20 }}>
            {Object.entries(d.cross_service_errors).map(([svc, msg]) => (
              <li key={svc}>
                <code>{svc}</code>: {msg}
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* Usage section */}
      <section style={{ marginBottom: 24 }}>
        <h3 style={{ marginBottom: 12 }}>{t('quota.usage_title')}</h3>
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))',
            gap: 12,
          }}
        >
          <StatCard label={t('quota.tokens_input')} value={formatNumber(d.usage.llm_tokens_input)} />
          <StatCard label={t('quota.tokens_output')} value={formatNumber(d.usage.llm_tokens_output)} />
          <StatCard label={t('quota.tokens_total')} value={formatNumber(d.usage.llm_tokens_total)} />
          <StatCard label={t('quota.cost')} value={formatCost(d.usage.llm_cost_usd)} />
          <StatCard label={t('quota.plugin_installs')} value={formatNumber(d.usage.plugin_installs)} />
        </div>
      </section>

      {/* Quota progress */}
      <section style={{ marginBottom: 24 }}>
        <h3 style={{ marginBottom: 12 }}>{t('quota.budget_title')}</h3>
        <div
          style={{
            padding: 16,
            background: 'var(--color-bg-secondary, #f9fafb)',
            border: '1px solid var(--color-border, #e5e7eb)',
            borderRadius: 8,
          }}
        >
          <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 8 }}>
            <span>
              {t('quota.used')}: <strong>{formatNumber(d.usage.llm_tokens_total)}</strong> /{' '}
              {formatNumber(d.quota.llm_tokens_monthly)}
            </span>
            <span style={{ color: progressColor(d.quota.percent_used) }}>
              <strong>{d.quota.percent_used.toFixed(1)}%</strong>
            </span>
          </div>
          <div
            style={{
              height: 10,
              background: 'var(--color-bg-tertiary, #e5e7eb)',
              borderRadius: 5,
              overflow: 'hidden',
            }}
          >
            <div
              style={{
                height: '100%',
                width: `${Math.min(100, d.quota.percent_used)}%`,
                background: progressColor(d.quota.percent_used),
                transition: 'width 200ms ease',
              }}
            />
          </div>
          <p style={{ marginTop: 8, fontSize: 13, color: 'var(--color-text-secondary)' }}>
            {t('quota.remaining')}: <strong>{formatNumber(d.quota.remaining)}</strong>{' '}
            {t('quota.tokens_unit')}
          </p>

          {showUpgrade && (
            <div style={{ marginTop: 16, padding: 12, background: 'var(--color-accent-bg, #dbeafe)', borderRadius: 6 }}>
              <p style={{ margin: '0 0 8px 0' }}>
                <strong>{t('quota.upgrade_prompt')}</strong>
              </p>
              <Button variant="primary" onClick={openUpgrade}>
                {t('quota.upgrade')}
              </Button>
            </div>
          )}
        </div>
      </section>

      {/* History */}
      <section>
        <h3 style={{ marginBottom: 12 }}>{t('quota.history_title')}</h3>
        {d.history.length === 0 ? (
          <p style={{ color: 'var(--color-text-secondary)', fontSize: 13 }}>{t('quota.history_empty')}</p>
        ) : (
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
            <thead>
              <tr style={{ borderBottom: '1px solid var(--color-border, #e5e7eb)' }}>
                <th style={{ textAlign: 'left', padding: '8px 4px' }}>{t('quota.col_month')}</th>
                <th style={{ textAlign: 'right', padding: '8px 4px' }}>{t('quota.col_tokens')}</th>
                <th style={{ textAlign: 'right', padding: '8px 4px' }}>{t('quota.col_cost')}</th>
              </tr>
            </thead>
            <tbody>
              {d.history.map((row) => (
                <tr key={row.month} style={{ borderBottom: '1px solid var(--color-border-light, #f3f4f6)' }}>
                  <td style={{ padding: '8px 4px' }}>{row.month}</td>
                  <td style={{ padding: '8px 4px', textAlign: 'right' }}>{formatNumber(row.llm_tokens_total)}</td>
                  <td style={{ padding: '8px 4px', textAlign: 'right' }}>{formatCost(row.llm_cost_usd)}</td>
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
    <div
      style={{
        padding: 12,
        background: 'var(--color-bg-secondary, #f9fafb)',
        border: '1px solid var(--color-border, #e5e7eb)',
        borderRadius: 6,
      }}
    >
      <div style={{ fontSize: 12, color: 'var(--color-text-secondary)' }}>{label}</div>
      <div style={{ fontSize: 18, fontWeight: 600, marginTop: 4 }}>{value}</div>
    </div>
  );
}
