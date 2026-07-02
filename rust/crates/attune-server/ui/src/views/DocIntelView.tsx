/** Document Intelligence view — compare / deep-summary / chapter-reading (T-10).
 *
 * Renders the spec §3.5 three output modes:
 *   - compare  → MARKED overlay: the source (doc b) text with changed/risk spans highlighted
 *     by annotation char-offset (offset→span), DiffVerdict color + hover note.
 *   - summarize → NARRATIVE: the layered report (overview + per-chapter bullets).
 *   - chapters → REVIEW/批阅: per-chapter margin annotations + Q&A whose answer carries
 *     citation offsets anchored back into the chapter text.
 *
 * Cost discipline (CLAUDE.md §Cost&Trigger Contract): a cost chip renders the token_bill
 * naive-vs-actual bar so the user SEES how much token was saved; tier-3 buttons show a
 * member-gated state when the user is not paid.
 *
 * i18n (project §i18n): every user-visible string goes through t(); no hardcoded CJK literal.
 */

import type { JSX } from 'preact';
import { useSignal } from '@preact/signals';
import { Button, ExportButton } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import { api, ApiError } from '../store/api';
import {
  documentArtifact,
  tableArtifact,
  type ExportArtifact,
  type ExportBlock,
  type ExportFormat,
} from '../hooks/useExport';

// ─── response shapes (mirror routes/documents.rs DocEnvelope) ───
interface Annotation {
  offsetStart: number;
  offsetEnd: number;
  kind: string;
  note: string;
  severity: number;
}
interface TokenBill {
  naiveBaselineTokens: number;
  extractiveKeptTokens: number;
  mapLlmTokens: { in: number; out: number; model: string };
  reduceLlmTokens: { in: number; out: number; model: string };
  cacheReadTokens: number;
  cacheHitChunks: number;
  newChunks: number;
  baselineModel: string;
}
interface DocEnvelope {
  outputMode: string;
  result: unknown;
  annotations?: Annotation[];
  narrative?: string;
  tokenBill: TokenBill;
}

type Tab = 'compare' | 'summarize' | 'chapters';

/** Turn a doc-intelligence result envelope into a downloadable export artifact
 *  (🆓 zero-cost). The narrative report → a Document (md/docx/pdf); an annotation
 *  set → a Table (xlsx/csv/md/pdf). Returns null when there is nothing to export. */
function buildExportFromEnvelope(
  env: DocEnvelope,
): { artifact: ExportArtifact; formats: ExportFormat[]; filename: string } | null {
  if (env.narrative && env.narrative.trim()) {
    const blocks: ExportBlock[] = env.narrative
      .split(/\n{2,}/)
      .map((p) => p.trim())
      .filter(Boolean)
      .map((text) => ({ kind: 'paragraph', text }));
    return {
      artifact: documentArtifact(blocks, t('docIntel.narrativeHeading')),
      formats: ['md', 'docx', 'pdf'],
      filename: 'attune-report',
    };
  }
  const anns = env.annotations ?? [];
  if (anns.length > 0) {
    const headers = [
      t('docIntel.export.colKind'),
      t('docIntel.export.colSeverity'),
      t('docIntel.export.colNote'),
    ];
    const rows = anns.map((a) => [a.kind, String(a.severity), a.note]);
    return {
      artifact: tableArtifact(headers, rows, { title: t('docIntel.export.tableTitle') }),
      formats: ['xlsx', 'csv', 'md', 'pdf'],
      filename: 'attune-annotations',
    };
  }
  return null;
}

function actualBillable(b: TokenBill): number {
  return (b.mapLlmTokens?.in ?? 0) + (b.mapLlmTokens?.out ?? 0) + (b.reduceLlmTokens?.in ?? 0) + (b.reduceLlmTokens?.out ?? 0);
}
function savingsPct(b: TokenBill): number {
  if (!b || b.naiveBaselineTokens === 0) return 0;
  return Math.max(0, Math.min(1, 1 - actualBillable(b) / b.naiveBaselineTokens)) * 100;
}

/** Annotation kind → colour for the marked overlay. */
const KIND_COLORS: Record<string, { bg: string; border: string }> = {
  'stance-reversal': { bg: '#fef2f2', border: '#f87171' },
  'numeric-change': { bg: '#fefce8', border: '#facc15' },
  'substantive': { bg: '#fef3c7', border: '#f59e0b' },
  'citation': { bg: '#ecfdf5', border: '#34d399' },
  'note': { bg: '#eff6ff', border: '#60a5fa' },
};

/** Render `text` with `annotations` (char-offset spans) highlighted. */
function renderOverlay(text: string, annotations: Annotation[]): JSX.Element {
  const chars = Array.from(text);
  const sorted = [...annotations]
    .filter((a) => a.offsetEnd <= chars.length && a.offsetStart < a.offsetEnd)
    .sort((a, b) => a.offsetStart - b.offsetStart);
  const parts: JSX.Element[] = [];
  let cursor = 0;
  sorted.forEach((ann, i) => {
    if (ann.offsetStart < cursor) return;
    if (ann.offsetStart > cursor) {
      parts.push(<span key={`p${i}`}>{chars.slice(cursor, ann.offsetStart).join('')}</span>);
    }
    const c = KIND_COLORS[ann.kind] ?? { bg: '#f1f5f9', border: '#94a3b8' };
    parts.push(
      <mark
        key={`a${i}`}
        style={{
          background: c.bg,
          borderBottom: `2px solid ${c.border}`,
          borderRadius: '2px',
          padding: '0 1px',
        }}
        title={ann.note || ann.kind}
      >
        {chars.slice(ann.offsetStart, ann.offsetEnd).join('')}
      </mark>,
    );
    cursor = ann.offsetEnd;
  });
  if (cursor < chars.length) {
    parts.push(<span key="tail">{chars.slice(cursor).join('')}</span>);
  }
  return (
    <pre
      style={{
        maxHeight: 480,
        overflow: 'auto',
        padding: 'var(--space-4)',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        fontSize: 'var(--text-sm)',
        fontFamily: 'var(--font-mono, monospace)',
        whiteSpace: 'pre-wrap',
        wordBreak: 'break-word',
        lineHeight: 1.7,
        margin: 0,
      }}
    >
      {parts}
    </pre>
  );
}

/** Cost chip — naive-vs-actual token bar + savings %. */
function CostChip({ bill }: { bill: TokenBill }): JSX.Element {
  const pct = savingsPct(bill);
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
      title={t('docIntel.costTitle')}
    >
      <span style={{ fontWeight: 600 }}>{t('docIntel.tokenSaved')}</span>
      <div
        style={{
          width: 80,
          height: 6,
          background: 'var(--color-border)',
          borderRadius: 3,
          overflow: 'hidden',
        }}
      >
        <div
          style={{
            width: `${100 - pct}%`,
            height: '100%',
            background: 'var(--color-accent)',
            transition: 'width var(--duration-base)',
          }}
        />
      </div>
      <span style={{ fontWeight: 600 }}>{pct.toFixed(0)}%</span>
      <span>
        {t('docIntel.naive')}: {bill.naiveBaselineTokens} · {t('docIntel.actual')}: {actualBillable(bill)}
        {bill.cacheHitChunks > 0 ? ` · ${t('docIntel.cacheHit')}: ${bill.cacheHitChunks}` : ''}
      </span>
    </div>
  );
}

/* ── shared styles ── */
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

const labelStyle: JSX.CSSProperties = {
  fontSize: 'var(--text-sm)',
  color: 'var(--color-text-muted)',
  marginBottom: 'var(--space-1)',
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

export function DocIntelView(): JSX.Element {
  const tab = useSignal<Tab>('summarize');
  const loading = useSignal(false);
  const memberGated = useSignal(false);

  // shared inputs
  const leftText = useSignal('');
  const rightText = useSignal('');
  const sourceText = useSignal('');
  const question = useSignal('');
  const chapterIdx = useSignal(0);

  // results
  const envelope = useSignal<DocEnvelope | null>(null);

  async function run(path: string, body: unknown): Promise<void> {
    loading.value = true;
    memberGated.value = false;
    envelope.value = null;
    try {
      const env = await api.post<DocEnvelope>(`/documents/${path}`, body);
      envelope.value = env;
    } catch (e) {
      let code = '';
      let message = '';
      if (e instanceof ApiError) {
        try {
          const parsed = JSON.parse(e.body) as { code?: string; error?: string };
          code = parsed.code ?? '';
          message = parsed.error ?? '';
        } catch {
          message = e.body;
        }
      } else if (e instanceof Error) {
        message = e.message;
      }
      if (code === 'membership-required') {
        memberGated.value = true;
        toast('error', t('docIntel.memberRequired'));
      } else {
        toast('error', message || t('docIntel.runFailed'));
      }
    } finally {
      loading.value = false;
    }
  }

  const env = envelope.value;

  return (
    <div style={{ padding: 'var(--space-5)', maxWidth: 1100, margin: '0 auto' }}>
      <header style={{ marginBottom: 'var(--space-4)' }}>
        <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0 }}>
          {t('docIntel.title')}
        </h2>
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
        {(['compare', 'summarize', 'chapters'] as Tab[]).map((tkey) => {
          const active = tab.value === tkey;
          return (
            <button
              key={tkey}
              role="tab"
              aria-selected={active}
              onClick={() => (tab.value = tkey)}
              style={{
                ...tabBtnBase,
                color: active ? 'var(--color-text)' : 'var(--color-text-muted)',
                fontWeight: active ? 600 : 400,
                borderBottomColor: active ? 'var(--color-accent)' : 'transparent',
              }}
            >
              {t(`docIntel.tab${tkey.charAt(0).toUpperCase() + tkey.slice(1)}`)}
            </button>
          );
        })}
      </div>

      {/* Panel body */}
      <div style={{ display: 'flex', gap: 'var(--space-5)', flexDirection: 'column' }}>
        {/* ── compare ── */}
        {tab.value === 'compare' && (
          <div style={{ display: 'flex', gap: 'var(--space-4)', flexDirection: 'column' }}>
            <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 'var(--space-4)' }}>
              <div style={{ display: 'flex', flexDirection: 'column' }}>
                <span style={labelStyle}>{t('docIntel.leftPlaceholder')}</span>
                <textarea
                  value={leftText.value}
                  placeholder={t('docIntel.leftPlaceholder')}
                  onInput={(e) => (leftText.value = (e.target as HTMLTextAreaElement).value)}
                  style={textareaStyle}
                />
              </div>
              <div style={{ display: 'flex', flexDirection: 'column' }}>
                <span style={labelStyle}>{t('docIntel.rightPlaceholder')}</span>
                <textarea
                  value={rightText.value}
                  placeholder={t('docIntel.rightPlaceholder')}
                  onInput={(e) => (rightText.value = (e.target as HTMLTextAreaElement).value)}
                  style={textareaStyle}
                />
              </div>
            </div>
            <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
              <Button
                variant="primary"
                loading={loading.value}
                disabled={loading.value}
                onClick={() =>
                  run('compare', { left: { text: leftText.value }, right: { text: rightText.value }, mode: 'semantic' })
                }
              >
                {t('docIntel.runCompare')}
              </Button>
            </div>
          </div>
        )}

        {/* ── summarize ── */}
        {tab.value === 'summarize' && (
          <div style={{ display: 'flex', gap: 'var(--space-4)', flexDirection: 'column' }}>
            <div style={{ display: 'flex', flexDirection: 'column' }}>
              <span style={labelStyle}>{t('docIntel.sourcePlaceholder')}</span>
              <textarea
                value={sourceText.value}
                placeholder={t('docIntel.sourcePlaceholder')}
                onInput={(e) => (sourceText.value = (e.target as HTMLTextAreaElement).value)}
                style={textareaStyle}
              />
            </div>
            <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
              <Button
                variant="primary"
                loading={loading.value}
                disabled={loading.value}
                onClick={() =>
                  run('summarize', { source: { text: sourceText.value }, level: 'standard' })
                }
              >
                {t('docIntel.runSummarize')}
              </Button>
            </div>
          </div>
        )}

        {/* ── chapters ── */}
        {tab.value === 'chapters' && (
          <div style={{ display: 'flex', gap: 'var(--space-4)', flexDirection: 'column' }}>
            <div style={{ display: 'flex', flexDirection: 'column' }}>
              <span style={labelStyle}>{t('docIntel.sourcePlaceholder')}</span>
              <textarea
                value={sourceText.value}
                placeholder={t('docIntel.sourcePlaceholder')}
                onInput={(e) => (sourceText.value = (e.target as HTMLTextAreaElement).value)}
                style={textareaStyle}
              />
            </div>
            <div style={{ display: 'flex', gap: 'var(--space-3)', alignItems: 'center', flexWrap: 'wrap' }}>
              <div style={{ display: 'flex', flexDirection: 'column' }}>
                <span style={labelStyle}>{t('docIntel.chapterIdx')}</span>
                <input
                  type="number"
                  value={chapterIdx.value}
                  min={0}
                  aria-label={t('docIntel.chapterIdx')}
                  onInput={(e) => (chapterIdx.value = Number((e.target as HTMLInputElement).value))}
                  style={{
                    width: 80,
                    padding: 'var(--space-2)',
                    border: '1px solid var(--color-border)',
                    borderRadius: 'var(--radius-md)',
                    background: 'var(--color-bg)',
                    color: 'var(--color-text)',
                    fontSize: 'var(--text-sm)',
                  }}
                />
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', flex: 1, minWidth: 200 }}>
                <span style={labelStyle}>{t('docIntel.questionPlaceholder')}</span>
                <input
                  type="text"
                  value={question.value}
                  placeholder={t('docIntel.questionPlaceholder')}
                  onInput={(e) => (question.value = (e.target as HTMLInputElement).value)}
                  style={{
                    padding: 'var(--space-2)',
                    border: '1px solid var(--color-border)',
                    borderRadius: 'var(--radius-md)',
                    background: 'var(--color-bg)',
                    color: 'var(--color-text)',
                    fontSize: 'var(--text-sm)',
                  }}
                />
              </div>
            </div>
            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
              <Button
                variant="secondary"
                loading={loading.value}
                disabled={loading.value}
                onClick={() => run('chapters', { text: sourceText.value, action: 'list' })}
              >
                {t('docIntel.listChapters')}
              </Button>
              <Button
                variant="primary"
                loading={loading.value}
                disabled={loading.value}
                onClick={() =>
                  run('chapters', { text: sourceText.value, action: 'ask', chapterIdx: chapterIdx.value, question: question.value })
                }
              >
                {t('docIntel.askChapter')}
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
              color: 'var(--color-text)',
            }}
          >
            {t('docIntel.memberGateNotice')}
          </div>
        )}

        {/* ── results ── */}
        {env && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 'var(--space-3)', flexWrap: 'wrap' }}>
              <CostChip bill={env.tokenBill} />
              {(() => {
                const built = buildExportFromEnvelope(env);
                if (!built) return null;
                return (
                  <ExportButton
                    artifact={built.artifact}
                    formats={built.formats}
                    filename={built.filename}
                  />
                );
              })()}
            </div>

            {/* compare → marked overlay on the source (right doc) */}
            {env.outputMode === 'marked' && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
                <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
                  {t('docIntel.markedHeading')}
                </h3>
                {renderOverlay(rightText.value, env.annotations ?? [])}
              </div>
            )}

            {/* summarize → narrative report */}
            {env.outputMode === 'narrative' && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
                <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
                  {t('docIntel.narrativeHeading')}
                </h3>
                <pre
                  style={{
                    padding: 'var(--space-4)',
                    background: 'var(--color-surface)',
                    border: '1px solid var(--color-border)',
                    borderRadius: 'var(--radius-md)',
                    fontSize: 'var(--text-sm)',
                    whiteSpace: 'pre-wrap',
                    lineHeight: 1.6,
                    margin: 0,
                  }}
                >
                  {env.narrative ?? ''}
                </pre>
              </div>
            )}

            {/* chapters review → margin annotations + citation anchors */}
            {env.outputMode === 'review' && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
                <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
                  {t('docIntel.reviewHeading')}
                </h3>
                {renderOverlay(sourceText.value, env.annotations ?? [])}
              </div>
            )}

            {/* structured JSON dump — fallback for unrecognized outputMode */}
            {env.outputMode === 'structured' && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
                <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
                  {t('docIntel.structuredHeading')}
                </h3>
                <pre
                  style={{
                    padding: 'var(--space-4)',
                    background: 'var(--color-surface)',
                    border: '1px solid var(--color-border)',
                    borderRadius: 'var(--radius-md)',
                    fontSize: 'var(--text-xs)',
                    fontFamily: 'var(--font-mono, monospace)',
                    overflow: 'auto',
                    margin: 0,
                  }}
                >
                  {JSON.stringify(env.result, null, 2)}
                </pre>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
