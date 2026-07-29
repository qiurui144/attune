/** Monitoring View · info watch + cross-source verification + deep research.
 *
 * UI patterns follow OfficeView conventions: header with h1+subtitle,
 * grid sidebar+detail, cards with consistent padding/border-radius, and
 * ALL styles inline (no CSS classes).
 *
 * i18n: every user-visible string goes through t().
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState } from '../components';
import { confirmDialog } from '../components/ConfirmModal';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import { ApiError, api } from '../store/api';

// ── types ──
interface Watch {
  id: string;
  label: string;
  keywords: string[];
  entities?: string[];
  source_ids?: string[];
  digest_period: string;
  llm_summary: boolean;
  notify?: boolean;
  match_threshold?: number | null;
  enabled: boolean;
  hit_count_pending: number;
  last_digested_at: string | null;
}

interface Hit {
  item_id: string;
  title: string;
  score: number;
  reasons: string[];
  dedup_group: string | null;
  created_at: string;
}

interface DigestCard {
  watch_id: string;
  kind: string;
  title: string;
  entries: DigestEntry[];
  llm_summary: string | null;
  created_at: string;
}

interface DigestEntry {
  item_id: string;
  title: string;
  preview: string;
  sources: string[];
  score?: number;
  reasons?: string[];
}

interface WatchesResponse {
  watches: Watch[];
}

interface DigestResponse {
  card?: DigestCard;
}

interface ResearchClaim {
  text: string;
  verification: 'multi_source_confirmed' | 'single_source' | 'conflicting';
  sources: { kind: string; reference: string }[];
}

interface ResearchResponse {
  claims: ResearchClaim[];
  report_markdown: string;
  degraded: boolean;
  web_disabled: boolean;
}

// ── shared inline styles ──
const containerStyle: JSX.CSSProperties = {
  padding: 'var(--space-6)',
  maxWidth: 1200,
  margin: '0 auto',
};

const inputStyle: JSX.CSSProperties = {
  width: '100%',
  padding: 'var(--space-2)',
  background: 'var(--color-bg)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  color: 'var(--color-text)',
  fontSize: 'var(--text-sm)',
  boxSizing: 'border-box',
};

const selectStyle: JSX.CSSProperties = {
  padding: 'var(--space-2)',
  fontSize: 'var(--text-sm)',
  background: 'var(--color-bg)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  color: 'var(--color-text)',
};

const cardStyle: JSX.CSSProperties = {
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  padding: 'var(--space-3) var(--space-4)',
};

const monitoringGridStyle: JSX.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 320px), 1fr))',
  gap: 'var(--space-4)',
  alignItems: 'start',
};

const costChipBase: JSX.CSSProperties = {
  fontSize: 'var(--text-xs)',
  padding: '1px 8px',
  borderRadius: 'var(--radius-full)',
  fontWeight: 500,
};

const costChip: Record<string, JSX.CSSProperties> = {
  free: { ...costChipBase, background: 'var(--color-surface)', color: 'var(--color-success)', border: '1px solid var(--color-border)' },
  cloud: { ...costChipBase, background: 'var(--color-warning-bg, #fef3c7)', color: 'var(--color-warning, #b45309)', border: '1px solid var(--color-border)' },
  multi: { ...costChipBase, background: 'var(--color-accent-bg, #e0f2fe)', color: 'var(--color-accent)', border: '1px solid var(--color-border)' },
};

const verdictColors: Record<string, JSX.CSSProperties> = {
  multi_source_confirmed: { color: 'var(--color-success)', fontWeight: 600 },
  single_source: { color: 'var(--color-text-secondary)' },
  conflicting: { color: 'var(--color-danger, #b91c1c)', fontWeight: 600 },
};

function formatScore(score: number): string {
  return `${Math.round(score * 100)}%`;
}

function monitoringErrorMessage(e: unknown): string {
  if (e instanceof ApiError) {
    try {
      const body = JSON.parse(e.body) as { code?: string; error?: string };
      switch (body.code) {
        case 'membership-required':
          return t('monitoring.error.membership_required');
        case 'cloud-llm-disabled':
          return t('monitoring.error.cloud_llm_disabled');
        case 'research-llm-unavailable':
          return t('monitoring.error.llm_unavailable');
        default:
          return body.error || t('monitoring.error.generic');
      }
    } catch {
      return t('monitoring.error.generic');
    }
  }
  return t('monitoring.error.generic');
}

export function MonitoringView(): JSX.Element {
  const watches = useSignal<Watch[]>([]);
  const loading = useSignal(true);
  const busy = useSignal(false);
  const selected = useSignal<string | null>(null);
  const hits = useSignal<Hit[]>([]);
  const digest = useSignal<DigestCard | null>(null);
  const showForm = useSignal(false);

  // new-watch form
  const fLabel = useSignal('');
  const fKeywords = useSignal('');
  const fAnchor = useSignal('');
  const fPeriod = useSignal('daily');
  const fLlm = useSignal(false);
  const fNotify = useSignal(false);
  const fThreshold = useSignal(0.35);

  // deep research form
  const rTopic = useSignal('');
  const rUseWeb = useSignal(false);
  const rBusy = useSignal(false);
  const research = useSignal<ResearchResponse | null>(null);

  useEffect(() => { void loadWatches(); }, []);

  async function loadWatches(): Promise<void> {
    loading.value = true;
    try {
      const list = await api.get<WatchesResponse | Watch[]>('/monitoring/watches');
      watches.value = Array.isArray(list) ? list : list.watches ?? [];
    } catch {
      watches.value = [];
    } finally {
      loading.value = false;
    }
  }

  async function openWatch(id: string): Promise<void> {
    selected.value = id;
    hits.value = [];
    digest.value = null;
    try {
      const r = await api.get<{ hits: Hit[] }>(`/monitoring/watches/${id}/hits`);
      hits.value = r.hits ?? [];
    } catch { /* best effort */ }
  }

  async function createWatch(): Promise<void> {
    const label = fLabel.value.trim();
    const keywords = fKeywords.value.split(',').map((s: string) => s.trim()).filter(Boolean);
    const anchorText = fAnchor.value.trim();
    if (!label) {
      toast('error', t('monitoring.watch.label_required'));
      return;
    }
    if (keywords.length === 0 && !anchorText) {
      toast('error', t('monitoring.watch.criteria_required'));
      return;
    }
    busy.value = true;
    try {
      await api.post('/monitoring/watches', {
        label,
        keywords,
        anchor_text: anchorText,
        digest_period: fPeriod.value,
        llm_summary: fLlm.value,
        notify: fNotify.value,
        match_threshold: fThreshold.value,
      });
      showForm.value = false;
      fLabel.value = '';
      fKeywords.value = '';
      fAnchor.value = '';
      fNotify.value = false;
      fThreshold.value = 0.35;
      toast('success', t('monitoring.watch.created', { name: label }));
      await loadWatches();
    } catch (e) {
      toast('error', monitoringErrorMessage(e));
    } finally {
      busy.value = false;
    }
  }

  async function deleteWatch(id: string): Promise<void> {
    busy.value = true;
    try {
      await api.delete(`/monitoring/watches/${id}`);
      if (selected.value === id) selected.value = null;
      toast('success', t('monitoring.watch.deleted'));
      await loadWatches();
    } catch (e) {
      toast('error', monitoringErrorMessage(e));
    } finally {
      busy.value = false;
    }
  }

  async function scanNow(): Promise<void> {
    busy.value = true;
    try {
      const res = await api.post<{ new_hits?: number }>('/monitoring/scan', {});
      toast('success', t('monitoring.scan.done', { count: String(res.new_hits ?? 0) }));
      await loadWatches();
      if (selected.value) await openWatch(selected.value);
    } catch {
      toast('error', t('monitoring.error.generic'));
    } finally {
      busy.value = false;
    }
  }

  async function buildDigest(watchId: string): Promise<void> {
    busy.value = true;
    try {
      const r = await api.post<DigestResponse | DigestCard>(`/monitoring/watches/${watchId}/digest`, {});
      const card = 'card' in r && r.card ? r.card : (r as DigestCard);
      await loadWatches();
      await openWatch(watchId);
      digest.value = card;
    } catch {
      toast('error', t('monitoring.error.generic'));
    } finally {
      busy.value = false;
    }
  }

  async function runResearch(): Promise<void> {
    if (!rTopic.value.trim()) return;
    rBusy.value = true;
    research.value = null;
    try {
      const r = await api.post<ResearchResponse>('/monitoring/research', {
        topic: rTopic.value.trim(),
        use_web: rUseWeb.value,
      });
      research.value = r;
    } catch (e) {
      toast('error', monitoringErrorMessage(e));
    } finally {
      rBusy.value = false;
    }
  }

  async function updateWatch(id: string, patch: Partial<Pick<Watch, 'enabled' | 'digest_period' | 'llm_summary' | 'notify' | 'match_threshold'>>): Promise<void> {
    busy.value = true;
    try {
      await api.patch<Watch>(`/monitoring/watches/${id}`, patch);
      await loadWatches();
    } catch (e) {
      toast('error', monitoringErrorMessage(e));
    } finally {
      busy.value = false;
    }
  }

  const sel = watches.value.find((w) => w.id === selected.value) ?? null;
  const selThreshold = sel?.match_threshold ?? 0.35;

  return (
    <div style={containerStyle}>
      <header style={{ marginBottom: 'var(--space-4)' }}>
        <h1 style={{ fontSize: 'var(--text-2xl)', fontWeight: 600, margin: 0 }}>
          {t('monitoring.title')}
        </h1>
        <p style={{ color: 'var(--color-text-muted)', marginTop: 'var(--space-2)' }}>
          {t('monitoring.subtitle')}
        </p>
        <p style={{ color: 'var(--color-text-muted)', fontSize: 'var(--text-xs)', marginTop: 'var(--space-1)' }}>
          <span style={costChip.free}>{t('monitoring.cost.free')}</span> {t('monitoring.cost.note')}
        </p>
      </header>

      <div style={{ display: 'flex', gap: 'var(--space-2)', marginBottom: 'var(--space-4)' }}>
        <Button onClick={() => (showForm.value = !showForm.value)}>{t('monitoring.watch.new')}</Button>
        <Button variant="ghost" onClick={scanNow} disabled={busy.value}>
          {t('monitoring.scan.now')}
        </Button>
      </div>

      {showForm.value && (
        <section style={{ ...cardStyle, marginBottom: 'var(--space-4)' }}>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
            <div style={{ display: 'flex', flexDirection: 'column' }}>
              <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-muted)', marginBottom: 'var(--space-1)' }}>
                {t('monitoring.watch.label')}
              </span>
              <input value={fLabel.value} onInput={(e) => (fLabel.value = (e.target as HTMLInputElement).value)}
                placeholder={t('monitoring.watch.label_ph')} style={inputStyle} />
            </div>
            <div style={{ display: 'flex', flexDirection: 'column' }}>
              <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-muted)', marginBottom: 'var(--space-1)' }}>
                {t('monitoring.watch.keywords')}
              </span>
              <input value={fKeywords.value} onInput={(e) => (fKeywords.value = (e.target as HTMLInputElement).value)}
                placeholder={t('monitoring.watch.keywords_ph')} style={inputStyle} />
            </div>
            <div style={{ display: 'flex', flexDirection: 'column' }}>
              <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-muted)', marginBottom: 'var(--space-1)' }}>
                {t('monitoring.watch.anchor')}
              </span>
              <input value={fAnchor.value} onInput={(e) => (fAnchor.value = (e.target as HTMLInputElement).value)}
                placeholder={t('monitoring.watch.anchor_ph')} style={inputStyle} />
            </div>
            <div style={{ display: 'flex', gap: 'var(--space-4)', alignItems: 'center', flexWrap: 'wrap' }}>
              <label style={{ fontSize: 'var(--text-sm)', display: 'flex', alignItems: 'center', gap: 'var(--space-1)' }}>
                {t('monitoring.watch.period')}{' '}
                <select value={fPeriod.value} onChange={(e) => (fPeriod.value = (e.target as HTMLSelectElement).value)} style={selectStyle}>
                  <option value="daily">{t('monitoring.watch.period.daily')}</option>
                  <option value="weekly">{t('monitoring.watch.period.weekly')}</option>
                  <option value="off">{t('monitoring.watch.period.off')}</option>
                </select>
              </label>
              <label style={{ fontSize: 'var(--text-sm)', display: 'flex', alignItems: 'center', gap: 'var(--space-1)' }}>
                <input type="checkbox" checked={fLlm.value} onChange={(e) => (fLlm.value = (e.target as HTMLInputElement).checked)} />
                {t('monitoring.watch.llm_summary')}
                {fLlm.value && <span style={{ marginLeft: 'var(--space-1)', ...costChip.cloud }}>{t('monitoring.cost.cloud')}</span>}
              </label>
              <label style={{ fontSize: 'var(--text-sm)', display: 'flex', alignItems: 'center', gap: 'var(--space-1)' }}>
                <input type="checkbox" checked={fNotify.value} onChange={(e) => (fNotify.value = (e.target as HTMLInputElement).checked)} />
                {t('monitoring.watch.notify')}
              </label>
              <label style={{ fontSize: 'var(--text-sm)', display: 'flex', alignItems: 'center', gap: 'var(--space-2)' }}>
                {t('monitoring.watch.threshold')}
                <input
                  type="range"
                  min="0.1"
                  max="0.9"
                  step="0.05"
                  value={fThreshold.value}
                  onInput={(e) => (fThreshold.value = Number((e.target as HTMLInputElement).value))}
                />
                <span style={{ color: 'var(--color-text-muted)' }}>{Math.round(fThreshold.value * 100)}%</span>
              </label>
            </div>
            <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
              <Button onClick={createWatch} disabled={busy.value}>{t('monitoring.watch.create')}</Button>
            </div>
          </div>
        </section>
      )}

      <div style={monitoringGridStyle}>
        {/* watch list sidebar */}
        <aside style={{ minWidth: 0 }}>
          {loading.value ? (
            <p style={{ color: 'var(--color-text-muted)' }}>{t('common.loading')}</p>
          ) : watches.value.length === 0 ? (
            <EmptyState title={t('monitoring.watch.empty_title')} description={t('monitoring.watch.empty_desc')} />
          ) : (
            <ul style={{ listStyle: 'none', padding: 0, margin: 0, display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
              {watches.value.map((w) => {
                const active = selected.value === w.id;
                return (
                  <li
                    key={w.id}
                    onClick={() => void openWatch(w.id)}
                    style={{
                      border: '1px solid var(--color-border)',
                      borderRadius: 'var(--radius-md)',
                      padding: 'var(--space-3)',
                      background: active ? 'var(--color-surface-hover)' : 'transparent',
                      cursor: 'pointer',
                      transition: 'background var(--duration-fast)',
                    }}
                  >
                    <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                      <strong style={{ fontSize: 'var(--text-sm)' }}>{w.label}</strong>
                      <button
                        aria-label={t('common.delete')}
                        onClick={async (e) => {
                          e.stopPropagation();
                          const ok = await confirmDialog({
                            title: t('confirm.title.deleteWatch'),
                            message: t('monitoring.watch.delete_confirm'),
                            danger: true,
                          });
                          if (ok) void deleteWatch(w.id);
                        }}
                        style={{ border: 'none', background: 'none', cursor: 'pointer', fontSize: 'var(--text-sm)' }}
                      >
                        🗑
                      </button>
                    </div>
                    <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)', marginTop: 'var(--space-1)' }}>
                      {t('monitoring.watch.pending', { count: String(w.hit_count_pending) })}
                      {' · '}
                      {t(`monitoring.watch.period.${w.digest_period}`)}
                      {w.llm_summary && <span style={{ marginLeft: 'var(--space-1)', ...costChip.cloud }}>{t('monitoring.cost.cloud')}</span>}
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </aside>

        {/* detail panel */}
        <section style={{ minWidth: 0 }}>
          {sel && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
              <section style={cardStyle}>
                <div style={{ display: 'flex', justifyContent: 'space-between', gap: 'var(--space-3)', alignItems: 'flex-start' }}>
                  <div>
                    <h2 style={{ fontSize: 'var(--text-lg)', fontWeight: 600, margin: 0 }}>{sel.label}</h2>
                    <div style={{ display: 'flex', gap: 'var(--space-2)', flexWrap: 'wrap', marginTop: 'var(--space-2)' }}>
                      <span style={costChip.free}>{t('monitoring.watch.pending', { count: String(sel.hit_count_pending) })}</span>
                      <span style={sel.enabled ? costChip.free : costChip.cloud}>
                        {sel.enabled ? t('monitoring.watch.enabled') : t('monitoring.watch.disabled')}
                      </span>
                      <span style={costChip.free}>{t(`monitoring.watch.period.${sel.digest_period}`)}</span>
                    </div>
                  </div>
                  <Button onClick={() => void buildDigest(sel.id)} disabled={busy.value}>
                    {t('monitoring.digest.build')}
                  </Button>
                </div>
                <div style={{ display: 'flex', gap: 'var(--space-3)', alignItems: 'center', flexWrap: 'wrap', marginTop: 'var(--space-3)', paddingTop: 'var(--space-3)', borderTop: '1px solid var(--color-border)' }}>
                  <label style={{ fontSize: 'var(--text-sm)', display: 'flex', alignItems: 'center', gap: 'var(--space-1)' }}>
                    <input type="checkbox" checked={sel.enabled} disabled={busy.value} onChange={(e) => void updateWatch(sel.id, { enabled: (e.target as HTMLInputElement).checked })} />
                    {t('monitoring.watch.enabled')}
                  </label>
                  <label style={{ fontSize: 'var(--text-sm)', display: 'flex', alignItems: 'center', gap: 'var(--space-1)' }}>
                    <input type="checkbox" checked={Boolean(sel.notify)} disabled={busy.value} onChange={(e) => void updateWatch(sel.id, { notify: (e.target as HTMLInputElement).checked })} />
                    {t('monitoring.watch.notify')}
                  </label>
                  <label style={{ fontSize: 'var(--text-sm)', display: 'flex', alignItems: 'center', gap: 'var(--space-1)' }}>
                    <input type="checkbox" checked={sel.llm_summary} disabled={busy.value} onChange={(e) => void updateWatch(sel.id, { llm_summary: (e.target as HTMLInputElement).checked })} />
                    {t('monitoring.watch.llm_summary')}
                  </label>
                  <label style={{ fontSize: 'var(--text-sm)', display: 'flex', alignItems: 'center', gap: 'var(--space-1)' }}>
                    {t('monitoring.watch.period')}{' '}
                    <select value={sel.digest_period} disabled={busy.value} onChange={(e) => void updateWatch(sel.id, { digest_period: (e.target as HTMLSelectElement).value })} style={selectStyle}>
                      <option value="daily">{t('monitoring.watch.period.daily')}</option>
                      <option value="weekly">{t('monitoring.watch.period.weekly')}</option>
                      <option value="off">{t('monitoring.watch.period.off')}</option>
                    </select>
                  </label>
                  <label style={{ fontSize: 'var(--text-sm)', display: 'flex', alignItems: 'center', gap: 'var(--space-2)', minWidth: 220 }}>
                    {t('monitoring.watch.threshold')}
                    <input
                      type="range"
                      min="0.1"
                      max="0.9"
                      step="0.05"
                      value={selThreshold}
                      disabled={busy.value}
                      onChange={(e) => void updateWatch(sel.id, { match_threshold: Number((e.target as HTMLInputElement).value) })}
                      style={{ flex: 1 }}
                    />
                    <span style={{ color: 'var(--color-text-muted)' }}>{Math.round(selThreshold * 100)}%</span>
                  </label>
                </div>
                <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))', gap: 'var(--space-3)', marginTop: 'var(--space-3)', fontSize: 'var(--text-sm)' }}>
                  <div style={{ minWidth: 0 }}>
                    <div style={{ color: 'var(--color-text-muted)', marginBottom: 4 }}>{t('monitoring.watch.keywords')}</div>
                    <div style={{ wordBreak: 'break-word' }}>{sel.keywords.length > 0 ? sel.keywords.join('、') : '-'}</div>
                  </div>
                  <div style={{ minWidth: 0 }}>
                    <div style={{ color: 'var(--color-text-muted)', marginBottom: 4 }}>{t('monitoring.watch.entities')}</div>
                    <div style={{ wordBreak: 'break-word' }}>{(sel.entities ?? []).length > 0 ? (sel.entities ?? []).join('、') : '-'}</div>
                  </div>
                  <div>
                    <div style={{ color: 'var(--color-text-muted)', marginBottom: 4 }}>{t('monitoring.watch.last_digest')}</div>
                    <div>{sel.last_digested_at ? sel.last_digested_at.slice(0, 16).replace('T', ' ') : '-'}</div>
                  </div>
                </div>
              </section>

              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                <h2 style={{ fontSize: 'var(--text-lg)', fontWeight: 600, margin: 0 }}>
                  {t('monitoring.hits.title')}
                </h2>
              </div>

              {hits.value.length === 0 ? (
                <p style={{ color: 'var(--color-text-muted)', fontSize: 'var(--text-sm)' }}>{t('monitoring.hits.empty')}</p>
              ) : (
                <ul style={{ listStyle: 'none', padding: 0, margin: 0, display: 'flex', flexDirection: 'column' }}>
                  {hits.value.map((h) => (
                    <li
                      key={h.item_id}
                      style={{
                        borderBottom: '1px solid var(--color-border)',
                        padding: 'var(--space-2) 0',
                      }}
                    >
                      <div style={{ display: 'flex', justifyContent: 'space-between' }}>
                        <span style={{ fontSize: 'var(--text-sm)' }}>{h.title}</span>
                        <span style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)' }}>
                          {formatScore(h.score)}
                        </span>
                      </div>
                      <div style={{ height: 6, background: 'var(--color-border)', borderRadius: 3, overflow: 'hidden', marginTop: 6 }}>
                        <div style={{ width: `${Math.min(100, Math.max(0, h.score * 100))}%`, height: '100%', background: 'var(--color-accent)' }} />
                      </div>
                      <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)', marginTop: 2 }}>
                        {h.reasons.length > 0 ? h.reasons.join(' · ') : t('monitoring.hits.reason_none')}
                        {h.dedup_group && (
                          <span style={{ marginLeft: 'var(--space-1)', ...costChip.multi }}>
                            {t('monitoring.hits.multi_source')}
                          </span>
                        )}
                      </div>
                    </li>
                  ))}
                </ul>
              )}

              {digest.value && <DigestCardView card={digest.value} />}
            </div>
          )}

          {/* deep research */}
          <section style={{ marginTop: 'var(--space-5)', borderTop: '1px solid var(--color-border)', paddingTop: 'var(--space-4)' }}>
            <h2 style={{ fontSize: 'var(--text-lg)', fontWeight: 600, margin: '0 0 var(--space-3) 0' }}>
              {t('monitoring.research.title')}
            </h2>
            <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'center', flexWrap: 'wrap' }}>
              <input
                value={rTopic.value}
                onInput={(e) => (rTopic.value = (e.target as HTMLInputElement).value)}
                placeholder={t('monitoring.research.topic_ph')}
                style={{ flex: 1, minWidth: 220, ...inputStyle }}
              />
              <label style={{ fontSize: 'var(--text-sm)', display: 'flex', alignItems: 'center', gap: 'var(--space-1)', whiteSpace: 'nowrap' }}>
                <input type="checkbox" checked={rUseWeb.value} onChange={(e) => (rUseWeb.value = (e.target as HTMLInputElement).checked)} />
                {t('monitoring.research.use_web')}
              </label>
              <Button onClick={runResearch} disabled={rBusy.value}>
                {rBusy.value ? t('monitoring.research.running') : t('monitoring.research.run')}
              </Button>
              <span style={costChip.cloud}>{t('monitoring.cost.cloud')}</span>
            </div>
            {research.value && <ResearchResult res={research.value} />}
          </section>
        </section>
      </div>
    </div>
  );
}

function DigestCardView({ card }: { card: DigestCard }): JSX.Element {
  return (
    <section style={{ ...cardStyle, marginTop: 'var(--space-2)' }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>{card.title}</h3>
        <span style={costChip.free}>{t('monitoring.digest.entries', { count: String(card.entries.length) })}</span>
      </div>
      {card.llm_summary ? (
        <div style={{ background: 'var(--color-surface)', borderRadius: 'var(--radius-md)', padding: 'var(--space-3)', marginTop: 'var(--space-2)' }}>
          <strong style={{ fontSize: 'var(--text-sm)' }}>{t('monitoring.digest.llm_summary')}</strong>
          <pre style={{ whiteSpace: 'pre-wrap', fontFamily: 'inherit', margin: 'var(--space-1) 0 0', fontSize: 'var(--text-sm)', lineHeight: 1.5 }}>
            {card.llm_summary}
          </pre>
        </div>
      ) : (
        <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)', marginTop: 'var(--space-2)' }}>
          {t('monitoring.digest.extractive_only')}
        </p>
      )}
      <ul style={{ listStyle: 'none', padding: 0, margin: 0, display: 'flex', flexDirection: 'column' }}>
        {card.entries.map((e) => (
          <li key={e.item_id} style={{ borderTop: '1px solid var(--color-border)', padding: 'var(--space-2) 0' }}>
            <div style={{ fontWeight: 600, fontSize: 'var(--text-sm)' }}>{e.title}</div>
            <div style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-muted)', marginTop: 2 }}>{e.preview}</div>
            {e.sources.length > 0 && (
              <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)', marginTop: 'var(--space-1)' }}>
                {t('monitoring.digest.sources')}: {e.sources.join(', ')}
                {e.sources.length > 1 && (
                  <span style={{ marginLeft: 'var(--space-1)', ...costChip.multi }}>{t('monitoring.hits.multi_source')}</span>
                )}
              </div>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

function ResearchResult({ res }: { res: ResearchResponse }): JSX.Element {
  return (
    <div style={{ marginTop: 'var(--space-3)', display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      {res.degraded && (
        <p style={{ color: 'var(--color-warning, #b8860b)', fontSize: 'var(--text-sm)', margin: 0 }}>
          {t('monitoring.research.degraded')}
        </p>
      )}
      {res.web_disabled && (
        <p style={{ color: 'var(--color-text-muted)', fontSize: 'var(--text-sm)', margin: 0 }}>
          {t('monitoring.research.web_disabled')}
        </p>
      )}
      {res.claims.length > 0 && (
        <section>
          <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: '0 0 var(--space-2) 0' }}>
            {t('monitoring.research.claims')}
          </h3>
          <ul style={{ listStyle: 'none', padding: 0, margin: 0, display: 'flex', flexDirection: 'column' }}>
            {res.claims.map((c, i) => (
              <li key={i} style={{ padding: 'var(--space-2) 0', borderBottom: '1px solid var(--color-border)' }}>
                <span style={verdictColors[c.verification] ?? {}}>
                  {t(`monitoring.verdict.${c.verification}`)}
                </span>{' '}
                <span style={{ fontSize: 'var(--text-sm)' }}>{c.text}</span>
                <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)', marginTop: 2 }}>
                  {c.sources.map((s) => `[${s.kind}] ${s.reference}`).join(' · ')}
                </div>
              </li>
            ))}
          </ul>
        </section>
      )}
      <section>
        <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: '0 0 var(--space-2) 0' }}>
          {t('monitoring.research.report')}
        </h3>
        <pre style={{
          whiteSpace: 'pre-wrap',
          fontFamily: 'inherit',
          fontSize: 'var(--text-sm)',
          lineHeight: 1.6,
          padding: 'var(--space-3)',
          background: 'var(--color-surface)',
          borderRadius: 'var(--radius-md)',
          margin: 0,
        }}>
          {res.report_markdown}
        </pre>
      </section>
    </div>
  );
}
