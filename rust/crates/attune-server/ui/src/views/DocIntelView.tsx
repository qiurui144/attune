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
import { useEffect } from 'preact/hooks';
import { Button, EmptyState, ExportButton } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import { api, ApiError } from '../store/api';
import { useFilePicker } from '../hooks/useFilePicker';
import { getItem, loadItems } from '../hooks/useItems';
import { items } from '../store/signals';
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
type DocSlot = 'left' | 'right' | 'source';
type DocRefPayload = { itemId?: string; text?: string; path?: string; name?: string };

const DOC_INTEL_ACCEPT = '.txt,.md,.markdown,.png,.jpg,.jpeg,.webp,.gif,.bmp,.tif,.tiff';

function basename(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

function isImageName(name: string): boolean {
  return /\.(png|jpe?g|webp|gif|bmp|tiff?)$/i.test(name);
}

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

const workbenchGridStyle: JSX.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 380px), 1fr))',
  gap: 'var(--space-5)',
  alignItems: 'start',
};

const panelStyle: JSX.CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  gap: 'var(--space-4)',
  minWidth: 0,
  padding: 'var(--space-4)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  background: 'var(--color-surface)',
};

const resultPanelStyle: JSX.CSSProperties = {
  ...panelStyle,
  minHeight: 420,
  background: 'var(--color-bg)',
};

const formBlockStyle: JSX.CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  gap: 'var(--space-2)',
  minWidth: 0,
};

const formActionsStyle: JSX.CSSProperties = {
  display: 'flex',
  justifyContent: 'flex-end',
  gap: 'var(--space-2)',
  flexWrap: 'wrap',
};

const inputStyle: JSX.CSSProperties = {
  padding: 'var(--space-2)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  background: 'var(--color-bg)',
  color: 'var(--color-text)',
  fontSize: 'var(--text-sm)',
  boxSizing: 'border-box',
};

export function DocIntelView(): JSX.Element {
  const tab = useSignal<Tab>('summarize');
  const loading = useSignal(false);
  const memberGated = useSignal(false);

  // shared inputs
  const leftText = useSignal('');
  const rightText = useSignal('');
  const sourceText = useSignal('');
  const leftItemId = useSignal('');
  const rightItemId = useSignal('');
  const sourceItemId = useSignal('');
  const leftPath = useSignal('');
  const rightPath = useSignal('');
  const sourcePath = useSignal('');
  const question = useSignal('');
  const chapterIdx = useSignal(0);
  const { picking, pickFiles } = useFilePicker();

  // results
  const envelope = useSignal<DocEnvelope | null>(null);

  useEffect(() => {
    void loadItems(200, 0);
  }, []);

  function slotSignals(slot: DocSlot) {
    if (slot === 'left') return { text: leftText, itemId: leftItemId, path: leftPath };
    if (slot === 'right') return { text: rightText, itemId: rightItemId, path: rightPath };
    return { text: sourceText, itemId: sourceItemId, path: sourcePath };
  }

  function buildDocRef(slot: DocSlot): DocRefPayload {
    const s = slotSignals(slot);
    if (s.itemId.value) return { itemId: s.itemId.value };
    if (s.path.value) return { path: s.path.value, name: basename(s.path.value) };
    return { text: s.text.value };
  }

  async function chooseItem(slot: DocSlot, itemId: string): Promise<void> {
    const s = slotSignals(slot);
    s.itemId.value = itemId;
    if (itemId) {
      s.path.value = '';
      s.text.value = '';
      const item = await getItem(itemId);
      if (item) s.text.value = item.content;
    }
  }

  async function chooseFile(slot: DocSlot): Promise<void> {
    const s = slotSignals(slot);
    const { paths, files } = await pickFiles({
      multiple: false,
      accept: DOC_INTEL_ACCEPT,
      title: t('docIntel.pickFile'),
    });
    const path = paths[0] ?? '';
    const file = files[0] ?? null;
    const name = path ? basename(path) : file?.name ?? '';
    if (!name) return;
    s.itemId.value = '';
    if (path && isImageName(name)) {
      s.path.value = path;
      s.text.value = '';
      return;
    }
    if (file && /\.(txt|md|markdown)$/i.test(file.name)) {
      try {
        s.text.value = await file.text();
        s.path.value = '';
      } catch {
        toast('error', t('docIntel.fileReadFailed'));
      }
      return;
    }
    toast('error', t('docIntel.unsupportedFile'));
  }

  function renderSourcePicker(slot: DocSlot): JSX.Element {
    const s = slotSignals(slot);
    return (
      <div style={{ display: 'grid', gridTemplateColumns: 'minmax(0, 1fr) auto', gap: 'var(--space-2)', alignItems: 'end' }}>
        <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-1)', minWidth: 0 }}>
          <span style={labelStyle}>{t('docIntel.itemSource')}</span>
          <select
            value={s.itemId.value}
            onChange={(e) => void chooseItem(slot, (e.target as HTMLSelectElement).value)}
            style={{ ...inputStyle, width: '100%' }}
          >
            <option value="">{t('docIntel.itemSourcePlaceholder')}</option>
            {items.value.map((item) => (
              <option key={item.id} value={item.id}>
                {item.title || item.id}
              </option>
            ))}
          </select>
        </label>
        <Button variant="secondary" size="sm" loading={picking.value} disabled={picking.value} onClick={() => void chooseFile(slot)}>
          {t('docIntel.pickFile')}
        </Button>
        {s.path.value && (
          <div style={{ gridColumn: '1 / -1', fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
            {t('docIntel.selectedPath', { path: basename(s.path.value) })}
          </div>
        )}
      </div>
    );
  }

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

  function renderForm(): JSX.Element {
    if (tab.value === 'compare') {
      return (
        <>
          {renderSourcePicker('left')}
          <div style={formBlockStyle}>
            <span style={labelStyle}>{t('docIntel.leftPlaceholder')}</span>
            <textarea
              value={leftText.value}
              placeholder={t('docIntel.leftPlaceholder')}
              onInput={(e) => {
                leftText.value = (e.target as HTMLTextAreaElement).value;
                leftItemId.value = '';
                leftPath.value = '';
              }}
              style={textareaStyle}
            />
          </div>
          {renderSourcePicker('right')}
          <div style={formBlockStyle}>
            <span style={labelStyle}>{t('docIntel.rightPlaceholder')}</span>
            <textarea
              value={rightText.value}
              placeholder={t('docIntel.rightPlaceholder')}
              onInput={(e) => {
                rightText.value = (e.target as HTMLTextAreaElement).value;
                rightItemId.value = '';
                rightPath.value = '';
              }}
              style={textareaStyle}
            />
          </div>
          <div style={formActionsStyle}>
            <Button
              variant="primary"
              loading={loading.value}
              disabled={loading.value}
              onClick={() =>
                run('compare', { left: buildDocRef('left'), right: buildDocRef('right'), mode: 'semantic' })
              }
            >
              {t('docIntel.runCompare')}
            </Button>
          </div>
        </>
      );
    }

    if (tab.value === 'summarize') {
      return (
        <>
          {renderSourcePicker('source')}
          <div style={formBlockStyle}>
            <span style={labelStyle}>{t('docIntel.sourcePlaceholder')}</span>
            <textarea
              value={sourceText.value}
              placeholder={t('docIntel.sourcePlaceholder')}
              onInput={(e) => {
                sourceText.value = (e.target as HTMLTextAreaElement).value;
                sourceItemId.value = '';
                sourcePath.value = '';
              }}
              style={{ ...textareaStyle, minHeight: 260 }}
            />
          </div>
          <div style={formActionsStyle}>
            <Button
              variant="primary"
              loading={loading.value}
              disabled={loading.value}
              onClick={() =>
                run('summarize', { source: buildDocRef('source'), level: 'standard' })
              }
            >
              {t('docIntel.runSummarize')}
            </Button>
          </div>
        </>
      );
    }

    return (
      <>
        {renderSourcePicker('source')}
        <div style={formBlockStyle}>
          <span style={labelStyle}>{t('docIntel.sourcePlaceholder')}</span>
          <textarea
            value={sourceText.value}
            placeholder={t('docIntel.sourcePlaceholder')}
            onInput={(e) => {
              sourceText.value = (e.target as HTMLTextAreaElement).value;
              sourceItemId.value = '';
              sourcePath.value = '';
            }}
            style={{ ...textareaStyle, minHeight: 220 }}
          />
        </div>
        <div style={{ display: 'grid', gridTemplateColumns: '96px minmax(0, 1fr)', gap: 'var(--space-3)', alignItems: 'end' }}>
          <div style={formBlockStyle}>
            <span style={labelStyle}>{t('docIntel.chapterIdx')}</span>
            <input
              type="number"
              value={chapterIdx.value}
              min={0}
              aria-label={t('docIntel.chapterIdx')}
              onInput={(e) => (chapterIdx.value = Number((e.target as HTMLInputElement).value))}
              style={{ ...inputStyle, width: '100%' }}
            />
          </div>
          <div style={formBlockStyle}>
            <span style={labelStyle}>{t('docIntel.questionPlaceholder')}</span>
            <input
              type="text"
              value={question.value}
              placeholder={t('docIntel.questionPlaceholder')}
              onInput={(e) => (question.value = (e.target as HTMLInputElement).value)}
              style={{ ...inputStyle, width: '100%' }}
            />
          </div>
        </div>
        <div style={formActionsStyle}>
          <Button
            variant="secondary"
            loading={loading.value}
            disabled={loading.value}
            onClick={() => run('chapters', { ...buildDocRef('source'), action: 'list' })}
          >
            {t('docIntel.listChapters')}
          </Button>
          <Button
            variant="primary"
            loading={loading.value}
            disabled={loading.value}
            onClick={() =>
              run('chapters', { ...buildDocRef('source'), action: 'ask', chapterIdx: chapterIdx.value, question: question.value })
            }
          >
            {t('docIntel.askChapter')}
          </Button>
        </div>
      </>
    );
  }

  function renderResult(): JSX.Element {
    if (!env) {
      return (
        <EmptyState
          icon="📄"
          title={t('docIntel.empty.title')}
          description={t('docIntel.empty.description')}
        />
      );
    }

    const built = buildExportFromEnvelope(env);

    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)', minWidth: 0 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 'var(--space-3)', flexWrap: 'wrap' }}>
          <CostChip bill={env.tokenBill} />
          {built && (
            <ExportButton
              artifact={built.artifact}
              formats={built.formats}
              filename={built.filename}
            />
          )}
        </div>

        {env.outputMode === 'marked' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)', minWidth: 0 }}>
            <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
              {t('docIntel.markedHeading')}
            </h3>
            {renderOverlay(rightText.value, env.annotations ?? [])}
          </div>
        )}

        {env.outputMode === 'narrative' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)', minWidth: 0 }}>
            <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
              {t('docIntel.narrativeHeading')}
            </h3>
            <pre
              style={{
                maxHeight: 560,
                overflow: 'auto',
                padding: 'var(--space-4)',
                background: 'var(--color-surface)',
                border: '1px solid var(--color-border)',
                borderRadius: 'var(--radius-md)',
                fontSize: 'var(--text-sm)',
                fontFamily: 'inherit',
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-word',
                lineHeight: 1.6,
                margin: 0,
              }}
            >
              {env.narrative ?? ''}
            </pre>
          </div>
        )}

        {env.outputMode === 'review' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)', minWidth: 0 }}>
            <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
              {t('docIntel.reviewHeading')}
            </h3>
            {renderOverlay(sourceText.value, env.annotations ?? [])}
          </div>
        )}

        {env.outputMode === 'structured' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)', minWidth: 0 }}>
            <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
              {t('docIntel.structuredHeading')}
            </h3>
            <pre
              style={{
                maxHeight: 560,
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
    );
  }

  return (
    <div style={{ padding: 'var(--space-5)', maxWidth: 1280, margin: '0 auto' }}>
      <header style={{ marginBottom: 'var(--space-4)' }}>
        <h1 style={{ fontSize: 'var(--text-2xl)', fontWeight: 600, margin: 0 }}>
          {t('docIntel.title')}
        </h1>
      </header>

      <div
        role="tablist"
        aria-label={t('docIntel.title')}
        style={{
          display: 'flex',
          gap: 'var(--space-2)',
          borderBottom: '1px solid var(--color-border)',
          marginBottom: 'var(--space-5)',
          overflowX: 'auto',
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
                whiteSpace: 'nowrap',
              }}
            >
              {t(`docIntel.tab${tkey.charAt(0).toUpperCase() + tkey.slice(1)}`)}
            </button>
          );
        })}
      </div>

      <div style={workbenchGridStyle}>
        <section style={panelStyle}>
          {renderForm()}
          {memberGated.value && (
            <div
              style={{
                padding: 'var(--space-3)',
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
        </section>

        <section style={resultPanelStyle}>
          {renderResult()}
        </section>
      </div>
    </div>
  );
}
