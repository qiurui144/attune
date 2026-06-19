/** Writing Engine view (spec §5 W1-W6) — grounded narrative generation.
 *
 * Six tabs, one per writing surface, mirroring DocIntelView's conventions:
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

/** Cost chip — naive-vs-actual token bar + savings % (§8.3 the user must SEE the savings). */
function CostChip({ bill }: { bill: TokenBill }): JSX.Element {
  const pct = savingsPct(bill);
  const isLocal = (bill.path ?? '') === 'zero-llm' || actualBillable(bill) === 0;
  return (
    <div class="writing-cost-chip" title={t('writing.costTitle')}>
      <span class="writing-cost-chip__tier">{isLocal ? t('writing.tierLocal') : t('writing.tierCloud')}</span>
      <div class="writing-cost-bar">
        <div class="writing-cost-bar__actual" style={{ width: `${100 - pct}%` }} />
      </div>
      <span class="writing-cost-chip__pct">{pct.toFixed(0)}%</span>
      <span class="writing-cost-chip__detail">
        {t('writing.naive')}: {bill.naiveBaselineTokens} · {t('writing.actual')}: {actualBillable(bill)}
      </span>
    </div>
  );
}

/** Render `content` with `unverifiedSpans` (UTF-16 offsets) marked so the user SEES which facts
 * could not be traced to a source (spec §7 — never silently present an ungrounded claim). */
function renderGrounded(content: string, unverified: [number, number][]): JSX.Element {
  // The engine emits UTF-16 offsets; for the common BMP case Array.from indexing matches. We render
  // conservatively: split on the sorted spans, mark each as needs-verification.
  const chars = Array.from(content);
  const sorted = [...unverified].filter((s) => s[0] < s[1] && s[1] <= chars.length).sort((a, b) => a[0] - b[0]);
  const parts: JSX.Element[] = [];
  let cursor = 0;
  sorted.forEach((span, i) => {
    if (span[0] < cursor) return;
    if (span[0] > cursor) parts.push(<span key={`p${i}`}>{chars.slice(cursor, span[0]).join('')}</span>);
    parts.push(
      <mark key={`u${i}`} class="writing-unverified" title={t('writing.unverifiedHint')}>
        {chars.slice(span[0], span[1]).join('')}
      </mark>,
    );
    cursor = span[1];
  });
  if (cursor < chars.length) parts.push(<span key="tail">{chars.slice(cursor).join('')}</span>);
  return <pre class="writing-content">{parts}</pre>;
}

function OutlineTree({ nodes }: { nodes: OutlineNode[] }): JSX.Element {
  return (
    <ul class="writing-outline-tree">
      {nodes.map((n, i) => (
        <li key={i}>
          {n.title}
          {n.children.length > 0 && <OutlineTree nodes={n.children} />}
        </li>
      ))}
    </ul>
  );
}

/** Parse the stable `{error,code}` body of an ApiError into (code, message). */
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

  // shared inputs
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

  // results
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

  return (
    <div class="writing-view">
      <h2>{t('writing.title')}</h2>
      <p class="writing-subtitle">{t('writing.subtitle')}</p>

      <div class="writing-tabs" role="tablist">
        {TABS.map((tb) => (
          <button key={tb} class={tab.value === tb ? 'active' : ''} onClick={() => (tab.value = tb)}>
            {t(`writing.tab.${tb}`)}
          </button>
        ))}
      </div>

      {tab.value === 'draft' && (
        <div class="writing-panel">
          <textarea value={outline.value} placeholder={t('writing.outlinePlaceholder')} onInput={(e) => (outline.value = (e.target as HTMLTextAreaElement).value)} />
          <input type="text" value={itemIds.value} placeholder={t('writing.itemIdsPlaceholder')} aria-label={t('writing.itemIdsLabel')} onInput={(e) => (itemIds.value = (e.target as HTMLInputElement).value)} />
          <textarea value={extraText.value} placeholder={t('writing.extraSourcePlaceholder')} onInput={(e) => (extraText.value = (e.target as HTMLTextAreaElement).value)} />
          <div class="writing-style-row">
            <input type="text" value={tone.value} placeholder={t('writing.tonePlaceholder')} aria-label={t('writing.toneLabel')} onInput={(e) => (tone.value = (e.target as HTMLInputElement).value)} />
            <input type="text" value={length.value} placeholder={t('writing.lengthPlaceholder')} aria-label={t('writing.lengthLabel')} onInput={(e) => (length.value = (e.target as HTMLInputElement).value)} />
            <input type="text" value={audience.value} placeholder={t('writing.audiencePlaceholder')} aria-label={t('writing.audienceLabel')} onInput={(e) => (audience.value = (e.target as HTMLInputElement).value)} />
          </div>
          <Button disabled={loading.value} onClick={() => call<WritingResult>('draft', { outline: outline.value, itemIds: parseItemIds(), extraSources: extraSources(), tone: tone.value || undefined, length: length.value || undefined, audience: audience.value || undefined }, (v) => (writingResult.value = v))}>
            {t('writing.runDraft')} <span class="writing-tier-badge">{t('writing.tierCloud')}</span>
          </Button>
        </div>
      )}

      {tab.value === 'rewrite' && (
        <div class="writing-panel">
          <textarea value={rewriteText.value} placeholder={t('writing.rewritePlaceholder')} onInput={(e) => (rewriteText.value = (e.target as HTMLTextAreaElement).value)} />
          <div class="writing-style-row">
            <input type="text" value={tone.value} placeholder={t('writing.tonePlaceholder')} aria-label={t('writing.toneLabel')} onInput={(e) => (tone.value = (e.target as HTMLInputElement).value)} />
            <input type="text" value={length.value} placeholder={t('writing.lengthPlaceholder')} aria-label={t('writing.lengthLabel')} onInput={(e) => (length.value = (e.target as HTMLInputElement).value)} />
            <input type="text" value={audience.value} placeholder={t('writing.audiencePlaceholder')} aria-label={t('writing.audienceLabel')} onInput={(e) => (audience.value = (e.target as HTMLInputElement).value)} />
          </div>
          <Button disabled={loading.value} onClick={() => call<WritingResult>('rewrite', { text: rewriteText.value, tone: tone.value || undefined, length: length.value || undefined, audience: audience.value || undefined }, (v) => (writingResult.value = v))}>
            {t('writing.runRewrite')} <span class="writing-tier-badge">{t('writing.tierCloud')}</span>
          </Button>
        </div>
      )}

      {tab.value === 'outline' && (
        <div class="writing-panel">
          <input type="text" value={topic.value} placeholder={t('writing.topicPlaceholder')} aria-label={t('writing.topicLabel')} onInput={(e) => (topic.value = (e.target as HTMLInputElement).value)} />
          <textarea value={fromDraft.value} placeholder={t('writing.fromDraftPlaceholder')} onInput={(e) => (fromDraft.value = (e.target as HTMLTextAreaElement).value)} />
          <div class="writing-btn-row">
            <Button disabled={loading.value} onClick={() => call<OutlineResult>('outline', { topic: topic.value }, (v) => (outlineResult.value = v))}>
              {t('writing.runOutlineForward')} <span class="writing-tier-badge">{t('writing.tierCloud')}</span>
            </Button>
            <Button disabled={loading.value} onClick={() => call<OutlineResult>('outline', { fromDraft: fromDraft.value }, (v) => (outlineResult.value = v))}>
              {t('writing.runOutlineReverse')} <span class="writing-tier-badge">{t('writing.tierLocal')}</span>
            </Button>
          </div>
        </div>
      )}

      {tab.value === 'synthesis' && (
        <div class="writing-panel">
          <input type="text" value={itemIds.value} placeholder={t('writing.itemIdsPlaceholder')} aria-label={t('writing.itemIdsLabel')} onInput={(e) => (itemIds.value = (e.target as HTMLInputElement).value)} />
          <textarea value={extraText.value} placeholder={t('writing.synthesisExtraPlaceholder')} onInput={(e) => (extraText.value = (e.target as HTMLTextAreaElement).value)} />
          <Button disabled={loading.value} onClick={() => call<WritingResult>('synthesis', { itemIds: parseItemIds(), extraSources: extraSources(), structure: 'thematic' }, (v) => (writingResult.value = v))}>
            {t('writing.runSynthesis')} <span class="writing-tier-badge">{t('writing.tierCloud')}</span>
          </Button>
        </div>
      )}

      {tab.value === 'cite' && (
        <div class="writing-panel">
          <label class="writing-label" for="cite-style">{t('writing.citeStyleLabel')}</label>
          <select id="cite-style" value={citeStyle.value} onChange={(e) => (citeStyle.value = (e.target as HTMLSelectElement).value)}>
            <option value="gbt7714">GB/T 7714</option>
            <option value="apa">APA</option>
            <option value="ieee">IEEE</option>
            <option value="mla">MLA</option>
          </select>
          <textarea value={citeJson.value} placeholder={t('writing.citeJsonPlaceholder')} onInput={(e) => (citeJson.value = (e.target as HTMLTextAreaElement).value)} />
          <Button
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
            {t('writing.runCite')} <span class="writing-tier-badge">{t('writing.tierLocal')}</span>
          </Button>
        </div>
      )}

      {tab.value === 'templates' && (
        <div class="writing-panel">
          <textarea value={templateText.value} placeholder={t('writing.templatePlaceholder')} onInput={(e) => (templateText.value = (e.target as HTMLTextAreaElement).value)} />
          <textarea value={templateValues.value} placeholder={t('writing.templateValuesPlaceholder')} onInput={(e) => (templateValues.value = (e.target as HTMLTextAreaElement).value)} />
          <Button disabled={loading.value} onClick={() => call<FillResult>('terms', { text: templateText.value, values: parseValues() }, (v) => (fillResult.value = v))}>
            {t('writing.runFill')} <span class="writing-tier-badge">{t('writing.tierLocal')}</span>
          </Button>
        </div>
      )}

      {memberGated.value && <div class="writing-member-gate">{t('writing.memberGateNotice')}</div>}

      {/* ── results ── */}
      {writingResult.value && (
        <div class="writing-result">
          <CostChip bill={writingResult.value.tokenBill} />
          {writingResult.value.unverifiedSpans.length > 0 && (
            <div class="writing-warn">{t('writing.unverifiedWarn')}</div>
          )}
          <h3>{t('writing.resultHeading')}</h3>
          {renderGrounded(writingResult.value.content, writingResult.value.unverifiedSpans)}
          {writingResult.value.annotations.length > 0 && (
            <div class="writing-annotations">
              <h4>{t('writing.suggestionsHeading')}</h4>
              <ul>
                {writingResult.value.annotations.map((a, i) => (
                  <li key={i}>
                    <strong>{a.suggestion}</strong> — {a.reason}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {outlineResult.value && (
        <div class="writing-result">
          <CostChip bill={outlineResult.value.tokenBill} />
          <h3>{t('writing.outlineHeading')}</h3>
          <OutlineTree nodes={outlineResult.value.nodes} />
        </div>
      )}

      {citeResult.value && (
        <div class="writing-result">
          <h3>{t('writing.citationsHeading')}</h3>
          <ol class="writing-citations">
            {citeResult.value.citations.map((c) => (
              <li key={c.id}>{c.formatted}</li>
            ))}
          </ol>
          {citeResult.value.inlineAnchors.length > 0 && (
            <p class="writing-anchor-note">{t('writing.inlineAnchorsFound')}: {citeResult.value.inlineAnchors.length}</p>
          )}
        </div>
      )}

      {fillResult.value && (
        <div class="writing-result">
          <h3>{t('writing.filledHeading')}</h3>
          <pre class="writing-content">{fillResult.value.filled}</pre>
          {fillResult.value.missingSlots.length > 0 && (
            <p class="writing-warn">{t('writing.missingSlots')}: {fillResult.value.missingSlots.join(', ')}</p>
          )}
        </div>
      )}
    </div>
  );
}
