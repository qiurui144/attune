/** Writing Engine view (spec §5 W1-W6) — grounded narrative generation.
 *
 * Six tabs, one per writing surface:
 *   - draft     (W1, 💰) outline + sources → grounded draft
 *   - rewrite   (W2, 💰) adjust tone/length/audience, fact-preserving
 *   - outline   (W3) forward (topic→tree, 💰) / reverse (draft→structure, 🆓)
 *   - synthesis (W5, 💰) multi-source map-reduce literature review
 *   - cite      (W4, 🆓) format references in 4 styles + inline anchors
 *   - templates (W6, 🆓) list templates + {{slot}} fill
 *
 * Cost discipline (CLAUDE.md §Cost&Trigger Contract): a cost chip shows the token_bill
 * naive-vs-actual + a tier badge (💰 cloud / 🆓 local) so the user SEES who pays BEFORE running;
 * tier-3 tabs surface a member-gated notice on 403.
 *
 * i18n (project §i18n): every user-visible string goes through t(); no hardcoded CJK literal.
 *
 * Style: ALL inline style={} per the project convention (OfficeView pattern). Zero CSS classes.
 */

import type { JSX } from 'preact';
import { useSignal } from '@preact/signals';
import { Button } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import { api, ApiError } from '../store/api';

// ─── response shapes (mirror attune-core writing types) ───
interface ModelLeg {
  in: number;
  out: number;
  model: string;
}
interface TokenBill {
  naiveBaselineTokens: number;
  extractiveKeptTokens: number;
  mapLlmTokens: ModelLeg;
  reduceLlmTokens: ModelLeg;
  cacheReadTokens: number;
  cacheHitChunks: number;
  newChunks: number;
  baselineModel: string;
  path: string;
}
interface GroundingRef {
  kind: string;
  itemId?: string;
  overlapTokens: number;
}
interface Segment {
  text: string;
  offset: [number, number];
  grounding: GroundingRef[];
  verified: boolean;
}
interface WritingResult {
  schemaVersion: number;
  mode: string;
  content: string;
  segments: Segment[];
  annotations: { offset: [number, number]; suggestion: string; reason: string }[];
  unverifiedSpans: [number, number][];
  tokenBill: TokenBill;
}
interface OutlineNode {
  title: string;
  children: OutlineNode[];
  sourceRef?: string;
}
interface OutlineResult {
  nodes: OutlineNode[];
  reverse: boolean;
  tokenBill: TokenBill;
}
interface Citation {
  id: string;
  seq: number;
  formatted: string;
  style: string;
}
interface CiteResponse {
  citations: Citation[];
  inlineAnchors: { offset: [number, number]; citationId: string }[];
}
interface FillResult {
  filled: string;
  missingSlots: string[];
  unusedValues: string[];
}

type Tab = 'draft' | 'rewrite' | 'outline' | 'synthesis' | 'cite' | 'templates';

function actualBillable(b: TokenBill): number {
  return (b.mapLlmTokens?.in ?? 0) + (b.mapLlmTokens?.out ?? 0) + (b.reduceLlmTokens?.in ?? 0) + (b.reduceLlmTokens?.out ?? 0);
}
function savingsPct(b: TokenBill): number {
  if (!b || b.naiveBaselineTokens === 0) return 0;
  return Math.max(0, Math.min(1, 1 - actualBillable(b) / b.naiveBaselineTokens)) * 100;
}

// ── shared inline styles ──────────────────────────────────────
const containerStyle: JSX.CSSProperties = {
  padding: 'var(--space-6)',
  maxWidth: 1200,
  margin: '0 auto',
};

const textareaStyle: JSX.CSSProperties = {
  width: '100%',
  minHeight: 160,
  padding: 'var(--space-3)',
  background: 'var(--color-bg)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  color: 'var(--color-text)',
  fontSize: 'var(--text-sm)',
  fontFamily: 'var(--font-mono, monospace)',
  resize: 'vertical',
  boxSizing: 'border-box',
};

const inputStyle: JSX.CSSProperties = {
  padding: 'var(--space-2)',
  background: 'var(--color-bg)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  color: 'var(--color-text)',
  fontSize: 'var(--text-sm)',
};

const selectStyle: JSX.CSSProperties = {
  ...inputStyle,
  minWidth: 200,
};

const labelStyle: JSX.CSSProperties = {
  fontSize: 'var(--text-sm)',
  color: 'var(--color-text-muted)',
  marginBottom: 'var(--space-1)',
};

const resultContainerStyle: JSX.CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  gap: 'var(--space-3)',
  padding: 'var(--space-4)',
  background: 'var(--color-surface)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-lg)',
  marginTop: 'var(--space-4)',
};

const preStyle: JSX.CSSProperties = {
  padding: 'var(--space-4)',
  background: 'var(--color-bg)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  fontSize: 'var(--text-sm)',
  fontFamily: 'var(--font-mono, monospace)',
  whiteSpace: 'pre-wrap',
  lineHeight: 1.7,
  margin: 0,
  maxHeight: 500,
  overflow: 'auto',
};

const tabBtnBase: JSX.CSSProperties = {
  padding: 'var(--space-3) var(--space-4)',
  border: 'none',
  background: 'transparent',
  fontSize: 'var(--text-sm)',
  fontWeight: 400,
  cursor: 'pointer',
  borderBottom: '2px solid transparent',
  marginBottom: -1,
  color: 'var(--color-text-muted)',
};

const warnStyle: JSX.CSSProperties = {
  padding: 'var(--space-2) var(--space-3)',
  background: 'var(--color-warning-bg, #fef3c7)',
  borderRadius: 'var(--radius-md)',
  fontSize: 'var(--text-sm)',
  color: 'var(--color-warning, #b45309)',
};

const tierBadge: JSX.CSSProperties = {
  fontSize: 'var(--text-xs)',
  padding: '1px 6px',
  borderRadius: 'var(--radius-full)',
  background: 'var(--color-surface-muted, #f3f4f6)',
  color: 'var(--color-text-secondary)',
  verticalAlign: 'middle',
};

/** Cost chip — naive-vs-actual token bar + savings %. */
function CostChip({ bill }: { bill: TokenBill }): JSX.Element {
  const pct = savingsPct(bill);
  const isLocal = (bill.path ?? '') === 'zero-llm' || actualBillable(bill) === 0;
  return (
    <div
      style={{
        display: 'flex',
        flexWrap: 'wrap',
        alignItems: 'center',
        gap: 'var(--space-2)',
        padding: 'var(--space-2) var(--space-3)',
        background: 'var(--color-surface-elevated)',
        borderRadius: 'var(--radius-md)',
        fontSize: 'var(--text-xs)',
        color: 'var(--color-text-secondary)',
      }}
      title={t('writing.costTitle')}
    >
      <span style={{ fontWeight: 600, ...tierBadge }}>
        {isLocal ? t('writing.tierLocal') : t('writing.tierCloud')}
      </span>
      <div style={{ width: 80, height: 6, background: 'var(--color-border)', borderRadius: 3, overflow: 'hidden' }}>
        <div style={{ width: `${100 - pct}%`, height: '100%', background: 'var(--color-accent)' }} />
      </div>
      <span style={{ fontWeight: 600 }}>{pct.toFixed(0)}%</span>
      <span>
        {t('writing.naive')}: {bill.naiveBaselineTokens} · {t('writing.actual')}: {actualBillable(bill)}
      </span>
    </div>
  );
}

/** Render content with unverifiedSpans (UTF-16 offsets) marked — spec §7 ground-truth visibility. */
function renderGrounded(content: string, unverified: [number, number][]): JSX.Element {
  const chars = Array.from(content);
  const sorted = [...unverified].filter((s) => s[0] < s[1] && s[1] <= chars.length).sort((a, b) => a[0] - b[0]);
  const parts: JSX.Element[] = [];
  let cursor = 0;
  sorted.forEach((span, i) => {
    if (span[0] < cursor) return;
    if (span[0] > cursor) parts.push(<span key={`p${i}`}>{chars.slice(cursor, span[0]).join('')}</span>);
    parts.push(
      <mark
        key={`u${i}`}
        style={{ background: '#fef2f2', borderBottom: '2px solid #f87171', borderRadius: 2, padding: '0 1px' }}
        title={t('writing.unverifiedHint')}
      >
        {chars.slice(span[0], span[1]).join('')}
      </mark>,
    );
    cursor = span[1];
  });
  if (cursor < chars.length) parts.push(<span key="tail">{chars.slice(cursor).join('')}</span>);
  return <pre style={preStyle}>{parts}</pre>;
}

function OutlineTree({ nodes }: { nodes: OutlineNode[] }): JSX.Element {
  return (
    <ul style={{ paddingLeft: 'var(--space-5)', margin: 0, fontSize: 'var(--text-sm)', lineHeight: 1.8 }}>
      {nodes.map((n, i) => (
        <li key={i}>
          {n.title}
          {n.children.length > 0 && <OutlineTree nodes={n.children} />}
        </li>
      ))}
    </ul>
  );
}

function parseErr(e: unknown): { code: string; message: string } {
  if (e instanceof ApiError) {
    try {
      const parsed = JSON.parse(e.body) as { code?: string; error?: string };
      return { code: parsed.code ?? '', message: parsed.error ?? e.body };
    } catch {
      return { code: '', message: e.body };
    }
  }
  if (e instanceof Error) return { code: '', message: e.message };
  return { code: '', message: String(e) };
}

export function WritingView(): JSX.Element {
  const tab = useSignal<Tab>('draft');
  const loading = useSignal(false);
  const memberGated = useSignal(false);

  const outline = useSignal('');
  const itemIds = useSignal('');
  const extraText = useSignal('');
  const tone = useSignal('');
  const length = useSignal('');
  const audience = useSignal('');
  const rewriteText = useSignal('');
  const topic = useSignal('');
  const fromDraft = useSignal('');
  const citeJson = useSignal('');
  const citeStyle = useSignal('apa');
  const templateText = useSignal('尊敬的{{name}}，关于{{topic}}……');
  const templateValues = useSignal('name=张三\ntopic=会议改期');

  const writingResult = useSignal<WritingResult | null>(null);
  const outlineResult = useSignal<OutlineResult | null>(null);
  const citeResult = useSignal<CiteResponse | null>(null);
  const fillResult = useSignal<FillResult | null>(null);

  function clearResults(): void {
    writingResult.value = null;
    outlineResult.value = null;
    citeResult.value = null;
    fillResult.value = null;
    memberGated.value = false;
  }

  async function call<T>(path: string, body: unknown, sink: (v: T) => void): Promise<void> {
    loading.value = true;
    clearResults();
    try {
      const v = await api.post<T>(`/api/v1/writing/${path}`, body);
      sink(v);
    } catch (e) {
      const { code, message } = parseErr(e);
      if (code === 'membership-required') {
        memberGated.value = true;
        toast('error', t('writing.memberRequired'));
      } else if (code === 'cloud-llm-disabled') {
        toast('error', t('writing.cloudDisabled'));
      } else {
        toast('error', message || t('writing.runFailed'));
      }
    } finally {
      loading.value = false;
    }
  }

  function parseItemIds(): string[] {
    return itemIds.value.split(/[,\s]+/).map((s) => s.trim()).filter(Boolean);
  }
  function extraSources(): { externalRef: string; text: string }[] {
    return extraText.value.trim() ? [{ externalRef: '', text: extraText.value }] : [];
  }
  function parseValues(): Record<string, string> {
    const out: Record<string, string> = {};
    templateValues.value.split('\n').forEach((line) => {
      const eq = line.indexOf('=');
      if (eq > 0) out[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
    });
    return out;
  }

  const TABS: Tab[] = ['draft', 'rewrite', 'outline', 'synthesis', 'cite', 'templates'];

  const panelGap: JSX.CSSProperties = { display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' };
  const btnRowRight: JSX.CSSProperties = { display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' };
  const inlineRow: JSX.CSSProperties = { display: 'flex', gap: 'var(--space-2)', flexWrap: 'wrap' };
  const fieldWide: JSX.CSSProperties = { display: 'flex', flexDirection: 'column', flex: 1, minWidth: 180 };

  return (
    <div style={containerStyle}>
      <header style={{ marginBottom: 'var(--space-4)' }}>
        <h2 style={{ fontSize: 'var(--text-2xl)', fontWeight: 600, margin: 0 }}>{t('writing.title')}</h2>
        <p style={{ color: 'var(--color-text-muted)', marginTop: 'var(--space-2)', fontSize: 'var(--text-sm)' }}>
          {t('writing.subtitle')}
        </p>
      </header>

      {/* Tab bar */}
      <div
        role="tablist"
        style={{
          display: 'flex',
          gap: 'var(--space-2)',
          borderBottom: '1px solid var(--color-border)',
          marginBottom: 'var(--space-4)',
        }}
      >
        {TABS.map((tb) => {
          const active = tab.value === tb;
          return (
            <button
              key={tb}
              role="tab"
              aria-selected={active}
              onClick={() => (tab.value = tb)}
              style={{
                ...tabBtnBase,
                color: active ? 'var(--color-text)' : 'var(--color-text-muted)',
                fontWeight: active ? 600 : 400,
                borderBottomColor: active ? 'var(--color-accent)' : 'transparent',
              }}
            >
              {t(`writing.tab.${tb}`)}
            </button>
          );
        })}
      </div>

      {/* ── draft (W1) ── */}
      {tab.value === 'draft' && (
        <div style={panelGap}>
          <textarea
            value={outline.value}
            placeholder={t('writing.outlinePlaceholder')}
            onInput={(e) => (outline.value = (e.target as HTMLTextAreaElement).value)}
            style={textareaStyle}
          />
          <input
            type="text"
            value={itemIds.value}
            placeholder={t('writing.itemIdsPlaceholder')}
            aria-label={t('writing.itemIdsLabel')}
            onInput={(e) => (itemIds.value = (e.target as HTMLInputElement).value)}
            style={inputStyle}
          />
          <textarea
            value={extraText.value}
            placeholder={t('writing.extraSourcePlaceholder')}
            onInput={(e) => (extraText.value = (e.target as HTMLTextAreaElement).value)}
            style={textareaStyle}
          />
          <div style={inlineRow}>
            <div style={fieldWide}>
              <span style={labelStyle}>{t('writing.toneLabel')}</span>
              <input type="text" value={tone.value} placeholder={t('writing.tonePlaceholder')}
                aria-label={t('writing.toneLabel')} onInput={(e) => (tone.value = (e.target as HTMLInputElement).value)} style={inputStyle} />
            </div>
            <div style={fieldWide}>
              <span style={labelStyle}>{t('writing.lengthLabel')}</span>
              <input type="text" value={length.value} placeholder={t('writing.lengthPlaceholder')}
                aria-label={t('writing.lengthLabel')} onInput={(e) => (length.value = (e.target as HTMLInputElement).value)} style={inputStyle} />
            </div>
            <div style={fieldWide}>
              <span style={labelStyle}>{t('writing.audienceLabel')}</span>
              <input type="text" value={audience.value} placeholder={t('writing.audiencePlaceholder')}
                aria-label={t('writing.audienceLabel')} onInput={(e) => (audience.value = (e.target as HTMLInputElement).value)} style={inputStyle} />
            </div>
          </div>
          <div style={btnRowRight}>
            <Button
              variant="primary"
              loading={loading.value}
              disabled={loading.value}
              onClick={() =>
                call<WritingResult>('draft', { outline: outline.value, itemIds: parseItemIds(), extraSources: extraSources(), tone: tone.value || undefined, length: length.value || undefined, audience: audience.value || undefined }, (v) => (writingResult.value = v))
              }
            >
              {t('writing.runDraft')} <span style={{ marginLeft: 'var(--space-1)' }}>💰</span>
            </Button>
          </div>
        </div>
      )}

      {/* ── rewrite (W2) ── */}
      {tab.value === 'rewrite' && (
        <div style={panelGap}>
          <textarea
            value={rewriteText.value}
            placeholder={t('writing.rewritePlaceholder')}
            onInput={(e) => (rewriteText.value = (e.target as HTMLTextAreaElement).value)}
            style={textareaStyle}
          />
          <div style={inlineRow}>
            <div style={fieldWide}>
              <span style={labelStyle}>{t('writing.toneLabel')}</span>
              <input type="text" value={tone.value} placeholder={t('writing.tonePlaceholder')}
                aria-label={t('writing.toneLabel')} onInput={(e) => (tone.value = (e.target as HTMLInputElement).value)} style={inputStyle} />
            </div>
            <div style={fieldWide}>
              <span style={labelStyle}>{t('writing.lengthLabel')}</span>
              <input type="text" value={length.value} placeholder={t('writing.lengthPlaceholder')}
                aria-label={t('writing.lengthLabel')} onInput={(e) => (length.value = (e.target as HTMLInputElement).value)} style={inputStyle} />
            </div>
            <div style={fieldWide}>
              <span style={labelStyle}>{t('writing.audienceLabel')}</span>
              <input type="text" value={audience.value} placeholder={t('writing.audiencePlaceholder')}
                aria-label={t('writing.audienceLabel')} onInput={(e) => (audience.value = (e.target as HTMLInputElement).value)} style={inputStyle} />
            </div>
          </div>
          <div style={btnRowRight}>
            <Button
              variant="primary"
              loading={loading.value}
              disabled={loading.value}
              onClick={() =>
                call<WritingResult>('rewrite', { text: rewriteText.value, tone: tone.value || undefined, length: length.value || undefined, audience: audience.value || undefined }, (v) => (writingResult.value = v))
              }
            >
              {t('writing.runRewrite')} <span style={{ marginLeft: 'var(--space-1)' }}>💰</span>
            </Button>
          </div>
        </div>
      )}

      {/* ── outline (W3) ── */}
      {tab.value === 'outline' && (
        <div style={panelGap}>
          <div style={{ ...inlineRow, alignItems: 'flex-end' }}>
            <div style={{ flex: 1 }}>
              <span style={labelStyle}>{t('writing.topicLabel')}</span>
              <input
                type="text"
                value={topic.value}
                placeholder={t('writing.topicPlaceholder')}
                aria-label={t('writing.topicLabel')}
                onInput={(e) => (topic.value = (e.target as HTMLInputElement).value)}
                style={{ ...inputStyle, width: '100%', boxSizing: 'border-box' }}
              />
            </div>
            <div style={{ flex: 1 }}>
              <span style={labelStyle}>{t('writing.fromDraftPlaceholder')}</span>
              <textarea
                value={fromDraft.value}
                placeholder={t('writing.fromDraftPlaceholder')}
                onInput={(e) => (fromDraft.value = (e.target as HTMLTextAreaElement).value)}
                style={{ ...textareaStyle, minHeight: 80, width: '100%', boxSizing: 'border-box' }}
              />
            </div>
          </div>
          <div style={btnRowRight}>
            <Button
              variant="primary"
              loading={loading.value}
              disabled={loading.value}
              onClick={() => call<OutlineResult>('outline', { topic: topic.value }, (v) => (outlineResult.value = v))}
            >
              {t('writing.runOutlineForward')} <span style={{ marginLeft: 'var(--space-1)' }}>💰</span>
            </Button>
            <Button
              variant="secondary"
              loading={loading.value}
              disabled={loading.value}
              onClick={() => call<OutlineResult>('outline', { fromDraft: fromDraft.value }, (v) => (outlineResult.value = v))}
            >
              {t('writing.runOutlineReverse')} 🆓
            </Button>
          </div>
        </div>
      )}

      {/* ── synthesis (W5) ── */}
      {tab.value === 'synthesis' && (
        <div style={panelGap}>
          <input
            type="text"
            value={itemIds.value}
            placeholder={t('writing.itemIdsPlaceholder')}
            aria-label={t('writing.itemIdsLabel')}
            onInput={(e) => (itemIds.value = (e.target as HTMLInputElement).value)}
            style={inputStyle}
          />
          <textarea
            value={extraText.value}
            placeholder={t('writing.synthesisExtraPlaceholder')}
            onInput={(e) => (extraText.value = (e.target as HTMLTextAreaElement).value)}
            style={textareaStyle}
          />
          <div style={btnRowRight}>
            <Button
              variant="primary"
              loading={loading.value}
              disabled={loading.value}
              onClick={() =>
                call<WritingResult>('synthesis', { itemIds: parseItemIds(), extraSources: extraSources(), structure: 'thematic' }, (v) => (writingResult.value = v))
              }
            >
              {t('writing.runSynthesis')} <span style={{ marginLeft: 'var(--space-1)' }}>💰</span>
            </Button>
          </div>
        </div>
      )}

      {/* ── cite (W4) ── */}
      {tab.value === 'cite' && (
        <div style={panelGap}>
          <div style={{ display: 'flex', flexDirection: 'column' }}>
            <span style={labelStyle}>{t('writing.citeStyleLabel')}</span>
            <select id="cite-style" value={citeStyle.value} onChange={(e) => (citeStyle.value = (e.target as HTMLSelectElement).value)} style={selectStyle}>
              <option value="gbt7714">GB/T 7714</option>
              <option value="apa">APA</option>
              <option value="ieee">IEEE</option>
              <option value="mla">MLA</option>
            </select>
          </div>
          <textarea
            value={citeJson.value}
            placeholder={t('writing.citeJsonPlaceholder')}
            onInput={(e) => (citeJson.value = (e.target as HTMLTextAreaElement).value)}
            style={textareaStyle}
          />
          <div style={btnRowRight}>
            <Button
              variant="primary"
              loading={loading.value}
              disabled={loading.value}
              onClick={() => {
                let sources: unknown;
                try {
                  sources = JSON.parse(citeJson.value || '[]');
                } catch {
                  toast('error', t('writing.citeJsonInvalid'));
                  return;
                }
                void call<CiteResponse>('cite', { sources, style: citeStyle.value }, (v) => (citeResult.value = v));
              }}
            >
              {t('writing.runCite')} 🆓
            </Button>
          </div>
        </div>
      )}

      {/* ── templates (W6) ── */}
      {tab.value === 'templates' && (
        <div style={panelGap}>
          <textarea
            value={templateText.value}
            placeholder={t('writing.templatePlaceholder')}
            onInput={(e) => (templateText.value = (e.target as HTMLTextAreaElement).value)}
            style={textareaStyle}
          />
          <textarea
            value={templateValues.value}
            placeholder={t('writing.templateValuesPlaceholder')}
            onInput={(e) => (templateValues.value = (e.target as HTMLTextAreaElement).value)}
            style={textareaStyle}
          />
          <div style={btnRowRight}>
            <Button
              variant="primary"
              loading={loading.value}
              disabled={loading.value}
              onClick={() => call<FillResult>('terms', { text: templateText.value, values: parseValues() }, (v) => (fillResult.value = v))}
            >
              {t('writing.runFill')} 🆓
            </Button>
          </div>
        </div>
      )}

      {/* ── member gate ── */}
      {memberGated.value && (
        <div
          style={{
            padding: 'var(--space-4)',
            background: 'rgba(212, 165, 116, 0.12)',
            border: '1px solid var(--color-warning)',
            borderRadius: 'var(--radius-md)',
            fontSize: 'var(--text-sm)',
            marginTop: 'var(--space-4)',
          }}
        >
          {t('writing.memberGateNotice')}
        </div>
      )}

      {/* ── writing results ── */}
      {writingResult.value && (
        <div style={resultContainerStyle}>
          <CostChip bill={writingResult.value.tokenBill} />
          {writingResult.value.unverifiedSpans.length > 0 && <div style={warnStyle}>{t('writing.unverifiedWarn')}</div>}
          <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>{t('writing.resultHeading')}</h3>
          {renderGrounded(writingResult.value.content, writingResult.value.unverifiedSpans)}
          {writingResult.value.annotations.length > 0 && (
            <div>
              <h4 style={{ fontSize: 'var(--text-sm)', fontWeight: 600, margin: '0 0 var(--space-2) 0' }}>
                {t('writing.suggestionsHeading')}
              </h4>
              <ul style={{ margin: 0, paddingLeft: 'var(--space-5)', fontSize: 'var(--text-sm)' }}>
                {writingResult.value.annotations.map((a, i) => (
                  <li key={i} style={{ marginBottom: 'var(--space-1)' }}>
                    <strong>{a.suggestion}</strong> — {a.reason}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {/* ── outline results ── */}
      {outlineResult.value && (
        <div style={resultContainerStyle}>
          <CostChip bill={outlineResult.value.tokenBill} />
          <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>{t('writing.outlineHeading')}</h3>
          <OutlineTree nodes={outlineResult.value.nodes} />
        </div>
      )}

      {/* ── cite results ── */}
      {citeResult.value && (
        <div style={resultContainerStyle}>
          <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>{t('writing.citationsHeading')}</h3>
          <ol style={{ margin: 0, paddingLeft: 'var(--space-5)', fontSize: 'var(--text-sm)', lineHeight: 1.8 }}>
            {citeResult.value.citations.map((c) => (
              <li key={c.id}>{c.formatted}</li>
            ))}
          </ol>
          {citeResult.value.inlineAnchors.length > 0 && (
            <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', margin: 0 }}>
              {t('writing.inlineAnchorsFound')}: {citeResult.value.inlineAnchors.length}
            </p>
          )}
        </div>
      )}

      {/* ── template fill results ── */}
      {fillResult.value && (
        <div style={resultContainerStyle}>
          <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>{t('writing.filledHeading')}</h3>
          <pre style={preStyle}>{fillResult.value.filled}</pre>
          {fillResult.value.missingSlots.length > 0 && (
            <div style={warnStyle}>{t('writing.missingSlots')}: {fillResult.value.missingSlots.join(', ')}</div>
          )}
        </div>
      )}
    </div>
  );
}
