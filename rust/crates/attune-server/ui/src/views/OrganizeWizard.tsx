/** OrganizeWizard · 文件夹一键智能整理 → 案卷 (Task 10)
 *
 * 流程：圈选范围(item_ids 或 bind_path) → POST /organize/analyze 得提案 →
 * 用户审核(改名 / 选 create|add-to|skip) → POST /organize/apply 批量归入案卷。
 *
 * 成本契约(CLAUDE.md §成本感知)：analyze 是显式用户触发；非付费会员仅得 tier-2
 * 抽取式命名(后端附 code=member-required-for-llm-label)，UI 据此提示可升级解锁 LLM 命名。
 *
 * attune-core 边界：UI 不解释 kind / role 取值，纯透传后端策略产物。
 */

import type { JSX } from 'preact';
import { useSignal } from '@preact/signals';
import { Button, Modal, toast } from '../components';
import { api } from '../store/api';
import { t } from '../i18n';

// ── 类型（与 attune-core organizer::types 序列化对齐） ─────────────────────────
interface GroupItem {
  item_id: string;
  title: string;
  role: string | null;
  role_confidence: number | null;
}

interface ProposalGroup {
  group_id: number;
  label: string;
  summary: string;
  confidence: number;
  label_source: 'Llm' | 'Extractive';
  suggested_kind: string;
  items: GroupItem[];
}

interface NoiseItem {
  item_id: string;
  title: string;
}

interface CostEstimate {
  tier: number;
  est_tokens: number;
  est_usd: number;
  model: string;
}

interface OrganizationProposal {
  proposal_id: string;
  corpus_domain: string | null;
  groups: ProposalGroup[];
  noise_items: NoiseItem[];
  cost: CostEstimate;
  dimension_mismatch_count: number;
  /** 后端非会员附：member-required-for-llm-label */
  code?: string;
}

interface ApplyResult {
  created: string[];
  updated: string[];
  filed_count: number;
  skipped_count: number;
  already_applied: boolean;
}

/** 每组的本地可编辑状态（不回写 proposal 对象，避免 signal 引用漂移）。 */
interface GroupEdit {
  label: string;
  action: 'create' | 'skip';
}

export type OrganizeWizardProps = {
  open: boolean;
  /** 按文件夹整理：传 bind_path 前缀。 */
  bindPath?: string;
  /** 按 item 列表整理：传 item_ids。两者择一。 */
  itemIds?: string[];
  onClose: () => void;
  /** apply 成功后回调（用于刷新 Projects 列表）。 */
  onDone?: () => void;
};

export function OrganizeWizard({
  open,
  bindPath,
  itemIds,
  onClose,
  onDone,
}: OrganizeWizardProps): JSX.Element | null {
  const proposal = useSignal<OrganizationProposal | null>(null);
  const edits = useSignal<Record<number, GroupEdit>>({});
  const analyzing = useSignal(false);
  const applying = useSignal(false);
  const error = useSignal<string | null>(null);
  // 当 caller 未传 bindPath / itemIds 时，让用户填一个待整理目录前缀。
  const pathInput = useSignal('');

  if (!open) return null;

  /** 是否需要用户手填路径（无 caller 传入的范围）。 */
  const needsPathInput = !bindPath && (!itemIds || itemIds.length === 0);

  const reset = (): void => {
    proposal.value = null;
    edits.value = {};
    error.value = null;
  };

  /** 解析本次 analyze 的 scope；needsPathInput 时用用户填的路径。 */
  const resolveScope = (): { bind_path: string } | { item_ids: string[] } | null => {
    if (bindPath) return { bind_path: bindPath };
    if (itemIds && itemIds.length > 0) return { item_ids: itemIds };
    const p = pathInput.value.trim();
    return p ? { bind_path: p } : null;
  };

  const close = (): void => {
    reset();
    onClose();
  };

  const onAnalyze = async (): Promise<void> => {
    const scope = resolveScope();
    if (!scope) {
      toast('error', t('organize.toast.scope_required'));
      return;
    }
    analyzing.value = true;
    error.value = null;
    try {
      const res = await api.post<OrganizationProposal>('/organize/analyze', { scope });
      proposal.value = res;
      const e: Record<number, GroupEdit> = {};
      for (const g of res.groups) {
        e[g.group_id] = { label: g.label, action: 'create' };
      }
      edits.value = e;
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      error.value = msg;
      toast('error', t('organize.toast.analyze_failed', { message: msg }));
    } finally {
      analyzing.value = false;
    }
  };

  const setEdit = (gid: number, patch: Partial<GroupEdit>): void => {
    const cur = edits.value[gid] ?? { label: '', action: 'create' };
    edits.value = { ...edits.value, [gid]: { ...cur, ...patch } };
  };

  const onApply = async (): Promise<void> => {
    const p = proposal.value;
    if (!p) return;
    applying.value = true;
    error.value = null;
    try {
      const confirmed_groups = p.groups.map((g) => {
        const ed = edits.value[g.group_id] ?? { label: g.label, action: 'create' as const };
        return {
          group_id: g.group_id,
          action: ed.action,
          title: ed.label.trim() || g.label,
          kind: g.suggested_kind,
          items: g.items.map((i) => ({ item_id: i.item_id, role: i.role ?? undefined })),
        };
      });
      const res = await api.post<ApplyResult>('/organize/apply', {
        proposal_id: p.proposal_id,
        confirmed_groups,
      });
      if (res.already_applied) {
        toast('info', t('organize.toast.already_applied'));
      } else {
        toast(
          'success',
          t('organize.toast.applied', {
            created: res.created.length,
            filed: res.filed_count,
          }),
        );
      }
      onDone?.();
      close();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      error.value = msg;
      toast('error', t('organize.toast.apply_failed', { message: msg }));
    } finally {
      applying.value = false;
    }
  };

  const p = proposal.value;

  return (
    <Modal open onClose={close} title={t('organize.title')} maxWidth={680}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)', minWidth: 420 }}>
        {error.value && (
          <div
            role="alert"
            style={{
              padding: 'var(--space-2) var(--space-3)',
              background: 'var(--color-error-bg, #ffe6e6)',
              color: 'var(--color-error, #c00)',
              border: '1px solid var(--color-border)',
              borderRadius: 'var(--radius-md)',
              fontSize: 'var(--text-sm)',
            }}
          >
            ⚠ {error.value}
          </div>
        )}

        {!p && (
          <>
            <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: 0 }}>
              {t('organize.intro')}
            </p>
            {needsPathInput && (
              <label
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 4,
                  fontSize: 'var(--text-sm)',
                  color: 'var(--color-text-secondary)',
                }}
              >
                <span>{t('organize.field.path')}</span>
                <input
                  type="text"
                  value={pathInput.value}
                  onInput={(e) => (pathInput.value = (e.currentTarget as HTMLInputElement).value)}
                  placeholder={t('organize.field.path_placeholder')}
                  autoFocus
                  style={{
                    padding: 'var(--space-2) var(--space-3)',
                    fontSize: 'var(--text-sm)',
                    background: 'var(--color-bg)',
                    border: '1px solid var(--color-border)',
                    borderRadius: 'var(--radius-md)',
                    color: 'var(--color-text)',
                    outline: 'none',
                  }}
                />
              </label>
            )}
            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
              <Button variant="secondary" size="sm" onClick={close}>
                {t('common.cancel')}
              </Button>
              <Button variant="primary" size="sm" onClick={() => void onAnalyze()} disabled={analyzing.value}>
                {analyzing.value ? t('organize.analyzing') : t('organize.analyze')}
              </Button>
            </div>
          </>
        )}

        {p && (
          <>
            <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', margin: 0 }}>
              {t('organize.cost', {
                tier: p.cost.tier,
                tokens: p.cost.est_tokens,
                usd: p.cost.est_usd.toFixed(4),
              })}
            </p>

            {p.code === 'member-required-for-llm-label' && (
              <div
                style={{
                  padding: 'var(--space-2) var(--space-3)',
                  background: 'var(--color-surface)',
                  border: '1px dashed var(--color-border)',
                  borderRadius: 'var(--radius-md)',
                  fontSize: 'var(--text-xs)',
                  color: 'var(--color-text-secondary)',
                }}
              >
                💡 {t('organize.member_hint')}
              </div>
            )}

            {p.groups.length === 0 ? (
              <div
                style={{
                  padding: 'var(--space-3)',
                  color: 'var(--color-text-secondary)',
                  fontSize: 'var(--text-sm)',
                }}
              >
                {t('organize.no_groups')}
              </div>
            ) : (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)', maxHeight: 420, overflow: 'auto' }}>
                {p.groups.map((g) => {
                  const ed = edits.value[g.group_id] ?? { label: g.label, action: 'create' };
                  const skipped = ed.action === 'skip';
                  return (
                    <div
                      key={g.group_id}
                      style={{
                        padding: 'var(--space-3)',
                        background: 'var(--color-surface)',
                        border: '1px solid var(--color-border)',
                        borderRadius: 'var(--radius-md)',
                        opacity: skipped ? 0.55 : 1,
                        display: 'flex',
                        flexDirection: 'column',
                        gap: 'var(--space-2)',
                      }}
                    >
                      <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)' }}>
                        <input
                          type="text"
                          value={ed.label}
                          onInput={(e) => setEdit(g.group_id, { label: (e.currentTarget as HTMLInputElement).value })}
                          aria-label={t('organize.field.group_name')}
                          disabled={skipped}
                          style={{
                            flex: 1,
                            padding: 'var(--space-2) var(--space-3)',
                            fontSize: 'var(--text-sm)',
                            background: 'var(--color-bg)',
                            border: '1px solid var(--color-border)',
                            borderRadius: 'var(--radius-md)',
                            color: 'var(--color-text)',
                            outline: 'none',
                          }}
                        />
                        <span
                          style={{
                            padding: '1px 6px',
                            fontSize: 'var(--text-xs)',
                            fontFamily: 'var(--font-mono)',
                            background: 'var(--color-bg)',
                            border: '1px solid var(--color-border)',
                            borderRadius: 'var(--radius-sm)',
                            color: 'var(--color-text-secondary)',
                          }}
                        >
                          {g.suggested_kind}
                        </span>
                      </div>

                      <div
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          gap: 'var(--space-3)',
                          fontSize: 'var(--text-xs)',
                          color: 'var(--color-text-secondary)',
                        }}
                      >
                        <span>
                          {t('organize.confidence')}: {Math.round(g.confidence * 100)}%
                        </span>
                        <span>{t('organize.item_count', { count: g.items.length })}</span>
                        <label style={{ display: 'flex', alignItems: 'center', gap: 4, cursor: 'pointer', marginLeft: 'auto' }}>
                          <input
                            type="checkbox"
                            checked={skipped}
                            onChange={(e) =>
                              setEdit(g.group_id, {
                                action: (e.currentTarget as HTMLInputElement).checked ? 'skip' : 'create',
                              })
                            }
                          />
                          <span>{t('organize.action.skip')}</span>
                        </label>
                      </div>

                      <ul
                        style={{
                          listStyle: 'none',
                          padding: 0,
                          margin: 0,
                          display: 'flex',
                          flexDirection: 'column',
                          gap: 2,
                          fontSize: 'var(--text-xs)',
                        }}
                      >
                        {g.items.map((i) => (
                          <li key={i.item_id} style={{ color: 'var(--color-text)' }}>
                            {i.title || i.item_id}
                            {i.role ? <span style={{ color: 'var(--color-text-secondary)' }}> · {i.role}</span> : null}
                          </li>
                        ))}
                      </ul>
                    </div>
                  );
                })}
              </div>
            )}

            {p.noise_items.length > 0 && (
              <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', margin: 0 }}>
                {t('organize.noise', { count: p.noise_items.length })}
              </p>
            )}

            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)', marginTop: 'var(--space-2)' }}>
              <Button variant="secondary" size="sm" onClick={() => reset()} disabled={applying.value}>
                {t('organize.reanalyze')}
              </Button>
              <Button
                variant="primary"
                size="sm"
                onClick={() => void onApply()}
                disabled={applying.value || p.groups.length === 0}
              >
                {applying.value ? t('organize.applying') : t('organize.confirm')}
              </Button>
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}
