/** AgentResultPanel · 变体 A —— agent 计算结果面板
 *
 * 通用 agent 结果展示（非 law 专属，遵守 OSS 边界）：基础事实**值默认显示、
 * 依据+修正默认收起、点击「依据」展开**；展开后是凭据原文卡片（多依据并列，冲突
 * 时顶冲突横幅）+ 来源标签 + 修正表单。必填字段由 schema 固定 → 缺失显式成红行，
 * 不会遗漏；缺必填项时计算阻断不估算。law-pro civil_loan_agent 是首个消费者。
 */

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { t } from '../i18n';

/** 一条凭据（原文摘录 + 定位）。 */
export type AgentCitation = {
  file: string;
  locator?: string;
  quote: string;
  /** 该证据指向的值 —— 多依据冲突时标注（如借条 50万 vs 流水 45万）。 */
  pointsTo?: string;
};

export type AgentFactSource = 'ai' | 'lawyer' | 'none';

/** 一个基础事实字段。 */
export type AgentFact = {
  field: string;
  label: string;
  /** 值；null = 未提取到（红行 + 待补）。 */
  value: string | null;
  citations: AgentCitation[];
  source: AgentFactSource;
  /** grounding 校验是否通过。 */
  verified: boolean;
  /** 多依据冲突说明（有则展开区顶冲突横幅）。 */
  conflict?: string;
  /** 红线 / 警告（如利率超 LPR 4 倍）。 */
  warning?: string;
};

export type AgentResult = {
  title: string;
  meta: string;
  facts: AgentFact[];
  /** 必填字段总数（清单由公式 schema 固定 → 漏在结构上不可能）。 */
  requiredCount: number;
  /** 计算结果；缺失 → 计算被阻断。 */
  computation?: { formula: string; rows: { label: string; value: string }[] };
  /** 计算阻断原因（缺哪些必填项）。 */
  blockedReason?: string;
};

export type AgentResultPanelProps = {
  result: AgentResult;
  /** 律师点「保存并重算」回调；宿主据此重跑 agent。 */
  onCorrect?: (field: string, newValue: string, note: string) => void;
};

const SOURCE_STYLE: Record<AgentFactSource, { bg: string; fg: string }> = {
  ai: { bg: '#eceefb', fg: '#5b6ad0' },
  lawyer: { bg: 'var(--color-accent-soft, #e7f0eb)', fg: 'var(--color-accent, #2f6f4f)' },
  none: { bg: '#f0f1f3', fg: 'var(--color-text-secondary, #6b7280)' },
};

function sourceLabel(s: AgentFactSource): string {
  return s === 'ai'
    ? t('agentpanel.source.ai')
    : s === 'lawyer'
      ? t('agentpanel.source.lawyer')
      : t('agentpanel.source.none');
}

/** 单个事实行 —— 收起态只显示值；点「依据」就地展开凭据 + 修正。 */
function FactRow({
  fact,
  onCorrect,
}: {
  fact: AgentFact;
  onCorrect?: AgentResultPanelProps['onCorrect'];
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const [fixing, setFixing] = useState(false);
  const [draft, setDraft] = useState(fact.value ?? '');
  const [note, setNote] = useState('');
  const missing = !fact.verified || fact.value == null;

  return (
    <div style={{ borderBottom: '1px solid var(--color-line, #eef0f2)' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '11px 16px' }}>
        <span style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-sm)', width: 78, flexShrink: 0 }}>
          {fact.label}
        </span>
        <span
          style={{
            fontSize: 'var(--text-base)',
            fontWeight: 600,
            color: missing ? 'var(--color-error, #b3431f)' : 'var(--color-text)',
          }}
        >
          {fact.value ?? t('agentpanel.notExtracted')}
        </span>
        {fact.conflict && (
          <span style={{ fontSize: 'var(--text-xs)', fontWeight: 600, color: 'var(--color-warning, #b3791f)', background: 'var(--color-warning-soft, #fbf1de)', padding: '1px 7px', borderRadius: 20 }}>
            ⚠ {t('agentpanel.conflict')}
          </span>
        )}
        {missing && !fact.conflict && (
          <span style={{ fontSize: 'var(--text-xs)', fontWeight: 600, color: 'var(--color-error, #b3431f)', background: 'var(--color-error-soft, #fbe6de)', padding: '1px 7px', borderRadius: 20 }}>
            ⚠ {t('agentpanel.pending')}
          </span>
        )}
        <span style={{ flex: 1 }} />
        <button
          type="button"
          onClick={() => setOpen(!open)}
          className="interactive"
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            background: 'var(--color-surface-hover, #f3f4f6)',
            border: '1px solid var(--color-border)',
            borderRadius: 20,
            padding: '2px 10px',
            cursor: 'pointer',
          }}
        >
          {t('agentpanel.evidence')}
          {fact.citations.length > 1 ? `·${fact.citations.length}` : ''} {open ? '▴' : '▾'}
        </button>
      </div>

      {open && (
        <div style={{ padding: '2px 16px 14px 92px', background: 'var(--color-bg, #fafbfc)' }}>
          <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-disabled, #9aa1a9)', margin: '8px 0 4px' }}>
            {t('agentpanel.citation')}
            {fact.citations.length > 1 ? `（${fact.citations.length}）` : ''}
          </div>
          {fact.citations.length === 0 && (
            <div style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-disabled)' }}>
              {t('agentpanel.noCitation')}
            </div>
          )}
          {fact.citations.map((c, i) => (
            <div
              key={i}
              style={{
                background: 'var(--color-surface)',
                border: '1px solid var(--color-border)',
                borderRadius: 7,
                padding: '8px 10px',
                marginBottom: 6,
              }}
            >
              <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8 }}>
                <span style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
                  📄 {c.file}
                  {c.locator ? ` · ${c.locator}` : ''}
                </span>
                {c.pointsTo && (
                  <span style={{ fontSize: 'var(--text-xs)', fontWeight: 600, color: 'var(--color-accent, #2f6f4f)', whiteSpace: 'nowrap' }}>
                    → {c.pointsTo}
                  </span>
                )}
              </div>
              <div style={{ fontSize: 'var(--text-sm)', marginTop: 3 }}>「{c.quote}」</div>
            </div>
          ))}
          {fact.conflict && (
            <div style={{ background: 'var(--color-warning-soft, #fbf1de)', border: '1px solid var(--color-warning, #b3791f)', borderRadius: 7, padding: '8px 10px', fontSize: 'var(--text-xs)', margin: '4px 0' }}>
              ⚠ {fact.conflict}
            </div>
          )}
          {fact.warning && (
            <div style={{ color: 'var(--color-warning, #b3791f)', background: 'var(--color-warning-soft, #fbf1de)', borderRadius: 6, padding: '6px 10px', fontSize: 'var(--text-xs)', marginTop: 4 }}>
              ⚠ {fact.warning}
            </div>
          )}
          <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-disabled)', margin: '8px 0 4px' }}>
            {t('agentpanel.source')}
          </div>
          <span style={{ fontSize: 'var(--text-xs)', fontWeight: 600, padding: '1px 8px', borderRadius: 20, ...sourceStyle(fact.source) }}>
            {sourceLabel(fact.source)}
          </span>
          {!fixing && (
            <div>
              <button
                type="button"
                onClick={() => {
                  setDraft(fact.value ?? '');
                  setFixing(true);
                }}
                className="interactive"
                style={{ marginTop: 9, fontSize: 'var(--text-xs)', color: 'var(--color-accent, #2f6f4f)', background: 'var(--color-accent-soft, #e7f0eb)', border: 'none', borderRadius: 6, padding: '5px 12px', cursor: 'pointer' }}
              >
                ✎ {t('agentpanel.correct')}
              </button>
            </div>
          )}
          {fixing && (
            <div style={{ marginTop: 8, background: 'var(--color-surface)', border: '1px solid var(--color-border)', borderRadius: 8, padding: 10 }}>
              <label style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-disabled)', display: 'block', marginBottom: 2 }}>
                {t('agentpanel.newValue')}
              </label>
              <input
                value={draft}
                onInput={(e) => setDraft(e.currentTarget.value)}
                style={{ width: '100%', fontSize: 'var(--text-sm)', padding: '5px 8px', border: '1px solid var(--color-border)', borderRadius: 6 }}
              />
              <label style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-disabled)', display: 'block', margin: '6px 0 2px' }}>
                {t('agentpanel.note')}
              </label>
              <input
                value={note}
                onInput={(e) => setNote(e.currentTarget.value)}
                style={{ width: '100%', fontSize: 'var(--text-sm)', padding: '5px 8px', border: '1px solid var(--color-border)', borderRadius: 6 }}
              />
              <div style={{ marginTop: 10, display: 'flex', gap: 8 }}>
                <button
                  type="button"
                  onClick={() => {
                    onCorrect?.(fact.field, draft, note);
                    setFixing(false);
                  }}
                  style={{ fontSize: 'var(--text-xs)', background: 'var(--color-accent, #2f6f4f)', color: '#fff', border: 'none', borderRadius: 6, padding: '6px 14px', cursor: 'pointer' }}
                >
                  {t('agentpanel.save')}
                </button>
                <button
                  type="button"
                  onClick={() => setFixing(false)}
                  style={{ fontSize: 'var(--text-xs)', background: 'transparent', color: 'var(--color-text-secondary)', border: 'none', cursor: 'pointer' }}
                >
                  {t('agentpanel.cancel')}
                </button>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function sourceStyle(s: AgentFactSource): Record<string, string> {
  const v = SOURCE_STYLE[s];
  return { background: v.bg, color: v.fg };
}

/** 变体 A 主面板。 */
export function AgentResultPanel({ result, onCorrect }: AgentResultPanelProps): JSX.Element {
  const ready = result.facts.filter((f) => f.verified && f.value != null).length;
  const pending = result.requiredCount - ready;

  return (
    <div style={{ background: 'var(--color-surface)', border: '1px solid var(--color-border)', borderRadius: 'var(--radius-md, 10px)', overflow: 'hidden' }}>
      <div style={{ padding: '14px 16px', borderBottom: '1px solid var(--color-line, #eef0f2)' }}>
        <div style={{ fontSize: 'var(--text-base)', fontWeight: 600 }}>{result.title}</div>
        <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', marginTop: 2 }}>{result.meta}</div>
      </div>

      {/* 完整度计数器 —— 清单由 schema 固定，缺失显式可见 */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '9px 16px', background: 'var(--color-bg, #fafbfc)', borderBottom: '1px solid var(--color-line)', fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
        📋 {t('agentpanel.required')}
        <span style={{ color: 'var(--color-accent, #2f6f4f)', fontWeight: 600 }}>
          {ready} {t('agentpanel.ready')}
        </span>
        {pending > 0 && (
          <>
            ·
            <span style={{ color: 'var(--color-error, #b3431f)', fontWeight: 600 }}>
              {pending} {t('agentpanel.pending')}
            </span>
          </>
        )}
      </div>

      <div style={{ fontSize: 'var(--text-xs)', fontWeight: 600, color: 'var(--color-text-disabled)', padding: '12px 16px 4px' }}>
        {t('agentpanel.facts')}
      </div>
      {result.facts.map((f) => (
        <FactRow key={f.field} fact={f} onCorrect={onCorrect} />
      ))}

      <div style={{ fontSize: 'var(--text-xs)', fontWeight: 600, color: 'var(--color-text-disabled)', padding: '12px 16px 4px' }}>
        {t('agentpanel.computation')}
      </div>
      {result.computation ? (
        <div style={{ padding: '4px 16px 14px', background: 'var(--color-bg, #fafbfc)' }}>
          <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', fontFamily: 'var(--font-mono, monospace)' }}>
            {result.computation.formula}
          </div>
          {result.computation.rows.map((r, i) => (
            <div key={i} style={{ display: 'flex', justifyContent: 'space-between', marginTop: 6 }}>
              <span style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-sm)' }}>{r.label}</span>
              <span style={{ fontSize: 'var(--text-lg, 17px)', fontWeight: 700, color: 'var(--color-accent, #2f6f4f)' }}>{r.value}</span>
            </div>
          ))}
        </div>
      ) : (
        <div style={{ padding: '10px 16px 14px', background: 'var(--color-error-soft, #fbe6de)', borderTop: '1px solid var(--color-error, #b3431f)' }}>
          <div style={{ fontSize: 'var(--text-sm)', fontWeight: 600, color: 'var(--color-error, #b3431f)' }}>
            ⛔ {t('agentpanel.blocked')}
          </div>
          <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', marginTop: 3 }}>
            {result.blockedReason ?? ''}
          </div>
        </div>
      )}
    </div>
  );
}
