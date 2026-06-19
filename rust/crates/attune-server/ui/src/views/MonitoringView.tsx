/** Information-monitoring view — watches, triage hits, digest cards, deep research
 * with cross-source verification (spec 2026-06-19-info-monitoring-loop §2-B/§2-F/§2-G).
 *
 * Cost contract (CLAUDE.md §Cost & Trigger Contract): the default digest is zero-cost
 * extractive; the "smart summary" and "deep research" actions carry a cost chip so the user
 * SEES who pays. Nothing here polls or auto-spends — scan/digest/research are user-clicked.
 *
 * Cross-source verification: each deep-research claim renders a verdict badge
 * (multi-source confirmed / single source / conflicting) with its grounded source list —
 * never a bare JSON dump.
 *
 * i18n (project §i18n): every user-visible string goes through t(); zh/en key sets are
 * kept identical (grep guard).
 */

import type { JSX } from 'preact';
import { useSignal } from '@preact/signals';
import { Button, EmptyState } from '../components';
import { toast } from '../components/Toast';
import { confirmDialog } from '../components/ConfirmModal';
import { t } from '../i18n';
import { api, ApiError } from '../store/api';

// ── response shapes (mirror routes/watches.rs) ──
interface WatchView {
  id: string;
  label: string;
  keywords: string[];
  entities: string[];
  source_ids: string[];
  digest_period: string;
  llm_summary: boolean;
  notify: boolean;
  enabled: boolean;
  last_digested_at: string | null;
  hit_count_pending: number;
}
interface HitView {
  item_id: string;
  title: string;
  score: number;
  reasons: string[];
  dedup_group: string | null;
  created_at: string;
}
interface DigestEntry {
  item_id: string;
  title: string;
  preview: string;
  sources: string[];
  score: number;
  reasons: string[];
}
interface DigestCard {
  watch_id: string;
  kind: string;
  title: string;
  entries: DigestEntry[];
  llm_summary: string | null;
  created_at: string;
}
interface DigestResponse {
  card_id: string | null;
  entries: number;
  llm_summary_queued: boolean;
  card?: DigestCard;
}
type Verdict = 'multi_source_confirmed' | 'single_source' | 'conflicting';
interface VerifiedClaim {
  text: string;
  verification: Verdict;
  sources: Array<{ kind: 'vault' | 'web'; reference: string }>;
}
interface ResearchResponse {
  report_markdown: string;
  claims: VerifiedClaim[];
  degraded: boolean;
  web_disabled: boolean;
}

function errMsg(e: unknown): string {
  return e instanceof ApiError ? e.message : t('monitoring.error.generic');
}

/** Verdict → badge color (verification accuracy is the north star; conflicting = warn). */
function verdictClass(v: Verdict): string {
  switch (v) {
    case 'multi_source_confirmed':
      return 'mon-verdict mon-verdict--confirmed';
    case 'conflicting':
      return 'mon-verdict mon-verdict--conflict';
    default:
      return 'mon-verdict mon-verdict--single';
  }
}

export function MonitoringView(): JSX.Element {
  const watches = useSignal<WatchView[]>([]);
  const loading = useSignal(true);
  const selected = useSignal<string | null>(null);
  const hits = useSignal<HitView[]>([]);
  const digest = useSignal<DigestCard | null>(null);
  const busy = useSignal(false);

  // create-watch form
  const showForm = useSignal(false);
  const fLabel = useSignal('');
  const fKeywords = useSignal('');
  const fAnchor = useSignal('');
  const fPeriod = useSignal('weekly');
  const fLlm = useSignal(false);

  // deep research
  const rTopic = useSignal('');
  const rUseWeb = useSignal(true);
  const research = useSignal<ResearchResponse | null>(null);
  const rBusy = useSignal(false);

  async function loadWatches(): Promise<void> {
    loading.value = true;
    try {
      const res = await api.get<{ watches: WatchView[] }>('/monitoring/watches');
      watches.value = res.watches;
    } catch (e) {
      toast('error', errMsg(e));
    } finally {
      loading.value = false;
    }
  }
  // initial load (guarded so it runs once per mount)
  if (loading.value && watches.value.length === 0) {
    void loadWatches();
  }

  async function createWatch(): Promise<void> {
    if (!fLabel.value.trim()) {
      toast('error', t('monitoring.watch.label_required'));
      return;
    }
    const keywords = fKeywords.value.split(',').map((s) => s.trim()).filter(Boolean);
    if (keywords.length === 0 && !fAnchor.value.trim()) {
      toast('error', t('monitoring.watch.criteria_required'));
      return;
    }
    busy.value = true;
    try {
      const w = await api.post<WatchView>('/monitoring/watches', {
        label: fLabel.value.trim(),
        keywords,
        anchor_text: fAnchor.value.trim(),
        digest_period: fPeriod.value,
        llm_summary: fLlm.value,
      });
      toast('success', t('monitoring.watch.created', { name: w.label }));
      showForm.value = false;
      fLabel.value = '';
      fKeywords.value = '';
      fAnchor.value = '';
      await loadWatches();
    } catch (e) {
      toast('error', errMsg(e));
    } finally {
      busy.value = false;
    }
  }

  async function deleteWatch(id: string): Promise<void> {
    const ok = await confirmDialog({
      title: t('monitoring.watch.new'),
      message: t('monitoring.watch.delete_confirm'),
      danger: true,
    });
    if (!ok) return;
    try {
      await api.delete(`/monitoring/watches/${id}`);
      toast('success', t('monitoring.watch.deleted'));
      if (selected.value === id) {
        selected.value = null;
        hits.value = [];
        digest.value = null;
      }
      await loadWatches();
    } catch (e) {
      toast('error', errMsg(e));
    }
  }

  async function scanNow(): Promise<void> {
    busy.value = true;
    try {
      const res = await api.post<{ new_hits: number }>('/monitoring/scan', {});
      toast('success', t('monitoring.scan.done', { count: String(res.new_hits) }));
      if (selected.value) await openWatch(selected.value);
      await loadWatches();
    } catch (e) {
      toast('error', errMsg(e));
    } finally {
      busy.value = false;
    }
  }

  async function openWatch(id: string): Promise<void> {
    selected.value = id;
    digest.value = null;
    try {
      const res = await api.get<{ hits: HitView[] }>(`/monitoring/watches/${id}/hits?limit=50`);
      hits.value = res.hits;
    } catch (e) {
      toast('error', errMsg(e));
    }
  }

  async function buildDigest(id: string): Promise<void> {
    busy.value = true;
    try {
      const res = await api.post<DigestResponse>(`/monitoring/watches/${id}/digest`, {});
      if (!res.card_id || !res.card) {
        toast('info', t('monitoring.digest.no_hits'));
        digest.value = null;
        return;
      }
      digest.value = res.card;
      await loadWatches(); // pending count drops after digest marks hits
    } catch (e) {
      toast('error', errMsg(e));
    } finally {
      busy.value = false;
    }
  }

  async function runResearch(): Promise<void> {
    if (!rTopic.value.trim()) return;
    rBusy.value = true;
    research.value = null;
    try {
      const res = await api.post<ResearchResponse>('/monitoring/research', {
        topic: rTopic.value.trim(),
        use_web: rUseWeb.value,
        watch_id: selected.value ?? undefined,
      });
      research.value = res;
    } catch (e) {
      toast('error', errMsg(e));
    } finally {
      rBusy.value = false;
    }
  }

  const sel = watches.value.find((w) => w.id === selected.value) ?? null;

  return (
    <div style={{ padding: '24px', maxWidth: 1100, margin: '0 auto' }}>
      <header style={{ marginBottom: 16 }}>
        <h1 style={{ margin: 0, fontSize: 22 }}>{t('monitoring.title')}</h1>
        <p style={{ color: 'var(--color-text-muted)', margin: '6px 0 0' }}>
          {t('monitoring.subtitle')}
        </p>
        <p style={{ color: 'var(--color-text-muted)', fontSize: 12, margin: '4px 0 0' }}>
          <span class="mon-chip mon-chip--free">{t('monitoring.cost.free')}</span> {t('monitoring.cost.note')}
        </p>
      </header>

      <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
        <Button onClick={() => (showForm.value = !showForm.value)}>{t('monitoring.watch.new')}</Button>
        <Button variant="ghost" onClick={scanNow} disabled={busy.value}>
          {t('monitoring.scan.now')}
        </Button>
      </div>

      {showForm.value && (
        <section style={{ border: '1px solid var(--color-border)', borderRadius: 10, padding: 16, marginBottom: 16 }}>
          <label style={{ display: 'block', marginBottom: 8 }}>
            <span style={{ display: 'block', fontSize: 13 }}>{t('monitoring.watch.label')}</span>
            <input
              value={fLabel.value}
              onInput={(e) => (fLabel.value = (e.target as HTMLInputElement).value)}
              placeholder={t('monitoring.watch.label_ph')}
              style={{ width: '100%', padding: 8 }}
            />
          </label>
          <label style={{ display: 'block', marginBottom: 8 }}>
            <span style={{ display: 'block', fontSize: 13 }}>{t('monitoring.watch.keywords')}</span>
            <input
              value={fKeywords.value}
              onInput={(e) => (fKeywords.value = (e.target as HTMLInputElement).value)}
              placeholder={t('monitoring.watch.keywords_ph')}
              style={{ width: '100%', padding: 8 }}
            />
          </label>
          <label style={{ display: 'block', marginBottom: 8 }}>
            <span style={{ display: 'block', fontSize: 13 }}>{t('monitoring.watch.anchor')}</span>
            <input
              value={fAnchor.value}
              onInput={(e) => (fAnchor.value = (e.target as HTMLInputElement).value)}
              placeholder={t('monitoring.watch.anchor_ph')}
              style={{ width: '100%', padding: 8 }}
            />
          </label>
          <div style={{ display: 'flex', gap: 16, alignItems: 'center', marginBottom: 12 }}>
            <label style={{ fontSize: 13 }}>
              {t('monitoring.watch.period')}{' '}
              <select value={fPeriod.value} onChange={(e) => (fPeriod.value = (e.target as HTMLSelectElement).value)}>
                <option value="daily">{t('monitoring.watch.period.daily')}</option>
                <option value="weekly">{t('monitoring.watch.period.weekly')}</option>
                <option value="off">{t('monitoring.watch.period.off')}</option>
              </select>
            </label>
            <label style={{ fontSize: 13 }}>
              <input
                type="checkbox"
                checked={fLlm.value}
                onChange={(e) => (fLlm.value = (e.target as HTMLInputElement).checked)}
              />{' '}
              {t('monitoring.watch.llm_summary')}{' '}
              {fLlm.value && <span class="mon-chip mon-chip--cloud">{t('monitoring.cost.cloud')}</span>}
            </label>
          </div>
          <Button onClick={createWatch} disabled={busy.value}>
            {t('monitoring.watch.create')}
          </Button>
        </section>
      )}

      <div style={{ display: 'grid', gridTemplateColumns: '320px 1fr', gap: 16 }}>
        {/* watch list */}
        <aside>
          {loading.value ? (
            <p style={{ color: 'var(--color-text-muted)' }}>{t('common.loading')}</p>
          ) : watches.value.length === 0 ? (
            <EmptyState title={t('monitoring.watch.empty_title')} description={t('monitoring.watch.empty_desc')} />
          ) : (
            <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
              {watches.value.map((w) => (
                <li
                  key={w.id}
                  style={{
                    border: '1px solid var(--color-border)',
                    borderRadius: 8,
                    padding: 12,
                    marginBottom: 8,
                    background: selected.value === w.id ? 'var(--color-surface-2, rgba(127,165,165,0.1))' : 'transparent',
                    cursor: 'pointer',
                  }}
                  onClick={() => void openWatch(w.id)}
                >
                  <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                    <strong>{w.label}</strong>
                    <button
                      aria-label={t('common.delete')}
                      onClick={(e) => {
                        e.stopPropagation();
                        void deleteWatch(w.id);
                      }}
                      style={{ border: 'none', background: 'none', cursor: 'pointer' }}
                    >
                      🗑
                    </button>
                  </div>
                  <div style={{ fontSize: 12, color: 'var(--color-text-muted)', marginTop: 4 }}>
                    {t('monitoring.watch.pending', { count: String(w.hit_count_pending) })}
                    {w.llm_summary && <span class="mon-chip mon-chip--cloud" style={{ marginLeft: 6 }}>{t('monitoring.cost.cloud')}</span>}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </aside>

        {/* detail: hits + digest */}
        <section>
          {sel && (
            <>
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
                <h2 style={{ fontSize: 16, margin: 0 }}>{t('monitoring.hits.title')}</h2>
                <Button onClick={() => void buildDigest(sel.id)} disabled={busy.value}>
                  {t('monitoring.digest.build')}
                </Button>
              </div>
              {hits.value.length === 0 ? (
                <p style={{ color: 'var(--color-text-muted)' }}>{t('monitoring.hits.empty')}</p>
              ) : (
                <ul style={{ listStyle: 'none', padding: 0, margin: '0 0 16px' }}>
                  {hits.value.map((h) => (
                    <li key={h.item_id} style={{ borderBottom: '1px solid var(--color-border)', padding: '8px 0' }}>
                      <div style={{ display: 'flex', justifyContent: 'space-between' }}>
                        <span>{h.title}</span>
                        <span style={{ fontSize: 12, color: 'var(--color-text-muted)' }}>{h.score.toFixed(2)}</span>
                      </div>
                      <div style={{ fontSize: 12, color: 'var(--color-text-muted)' }}>
                        {h.reasons.join(' · ')}
                        {h.dedup_group && (
                          <span class="mon-chip mon-chip--multi" style={{ marginLeft: 6 }}>
                            {t('monitoring.hits.multi_source')}
                          </span>
                        )}
                      </div>
                    </li>
                  ))}
                </ul>
              )}

              {digest.value && <DigestCardView card={digest.value} />}
            </>
          )}

          {/* deep research */}
          <section style={{ marginTop: 24, borderTop: '1px solid var(--color-border)', paddingTop: 16 }}>
            <h2 style={{ fontSize: 16, marginTop: 0 }}>{t('monitoring.research.title')}</h2>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
              <input
                value={rTopic.value}
                onInput={(e) => (rTopic.value = (e.target as HTMLInputElement).value)}
                placeholder={t('monitoring.research.topic_ph')}
                style={{ flex: 1, minWidth: 220, padding: 8 }}
              />
              <label style={{ fontSize: 13 }}>
                <input
                  type="checkbox"
                  checked={rUseWeb.value}
                  onChange={(e) => (rUseWeb.value = (e.target as HTMLInputElement).checked)}
                />{' '}
                {t('monitoring.research.use_web')}
              </label>
              <Button onClick={runResearch} disabled={rBusy.value}>
                {rBusy.value ? t('monitoring.research.running') : t('monitoring.research.run')}
              </Button>
              <span class="mon-chip mon-chip--cloud">{t('monitoring.cost.cloud')}</span>
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
    <section style={{ border: '1px solid var(--color-border)', borderRadius: 10, padding: 16, marginTop: 8 }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <h3 style={{ margin: 0, fontSize: 15 }}>{card.title}</h3>
        <span class="mon-chip mon-chip--free">{t('monitoring.digest.entries', { count: String(card.entries.length) })}</span>
      </div>
      {card.llm_summary ? (
        <div style={{ background: 'var(--color-surface-2, rgba(127,165,165,0.08))', borderRadius: 8, padding: 12, margin: '10px 0' }}>
          <strong style={{ fontSize: 13 }}>{t('monitoring.digest.llm_summary')}</strong>
          <pre style={{ whiteSpace: 'pre-wrap', fontFamily: 'inherit', margin: '6px 0 0', fontSize: 13 }}>{card.llm_summary}</pre>
        </div>
      ) : (
        <p style={{ fontSize: 12, color: 'var(--color-text-muted)' }}>{t('monitoring.digest.extractive_only')}</p>
      )}
      <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
        {card.entries.map((e) => (
          <li key={e.item_id} style={{ borderTop: '1px solid var(--color-border)', padding: '8px 0' }}>
            <div style={{ fontWeight: 600 }}>{e.title}</div>
            <div style={{ fontSize: 13, color: 'var(--color-text-muted)' }}>{e.preview}</div>
            {e.sources.length > 0 && (
              <div style={{ fontSize: 11, color: 'var(--color-text-muted)', marginTop: 4 }}>
                {t('monitoring.digest.sources')}: {e.sources.join(', ')}
                {e.sources.length > 1 && (
                  <span class="mon-chip mon-chip--multi" style={{ marginLeft: 6 }}>{t('monitoring.hits.multi_source')}</span>
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
    <div style={{ marginTop: 12 }}>
      {res.degraded && (
        <p style={{ color: 'var(--color-warning, #b8860b)', fontSize: 13 }}>{t('monitoring.research.degraded')}</p>
      )}
      {res.web_disabled && (
        <p style={{ color: 'var(--color-text-muted)', fontSize: 13 }}>{t('monitoring.research.web_disabled')}</p>
      )}
      {res.claims.length > 0 && (
        <section style={{ marginBottom: 12 }}>
          <h3 style={{ fontSize: 14, margin: '0 0 6px' }}>{t('monitoring.research.claims')}</h3>
          <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
            {res.claims.map((c, i) => (
              <li key={i} style={{ padding: '6px 0', borderBottom: '1px solid var(--color-border)' }}>
                <span class={verdictClass(c.verification)}>{t(`monitoring.verdict.${c.verification}`)}</span>{' '}
                <span>{c.text}</span>
                <div style={{ fontSize: 11, color: 'var(--color-text-muted)', marginTop: 2 }}>
                  {c.sources.map((s) => `[${s.kind}] ${s.reference}`).join(' · ')}
                </div>
              </li>
            ))}
          </ul>
        </section>
      )}
      <section>
        <h3 style={{ fontSize: 14, margin: '0 0 6px' }}>{t('monitoring.research.report')}</h3>
        <pre style={{ whiteSpace: 'pre-wrap', fontFamily: 'inherit', fontSize: 13 }}>{res.report_markdown}</pre>
      </section>
    </div>
  );
}
