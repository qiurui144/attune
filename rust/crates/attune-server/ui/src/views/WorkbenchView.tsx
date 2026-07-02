/** WorkbenchView · 行业场景直接入口（工作台）
 *
 * 解决 chat 关键词触发不可靠的可发现性问题：每装一个 pro 插件，UI 自动渲染该
 * 行业的「场景卡片」。点击卡片 → 有表单则弹 PluginForm（复用 /forms schema 渲染）
 * → 提交直接 POST /agents/{id}/run，**不经 chat 意图猜测**；无表单则用通用输入。
 *
 * 入口全部从后端 /api/v1/scenarios 派生（plugin.yaml 声明驱动）；本视图零行业硬编码，
 * 装插件即出卡，符合 OSS 边界（通用渲染器在 OSS，行业内容在 attune-pro plugin.yaml）。
 *
 * chat 触发保留为补充入口（不删，spec §10）；工作台是新增主入口。
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState, Modal, PluginForm, SuggestionsPanel, toast } from '../components';
import { api } from '../store/api';
import { currentView, settingsInitialTab, drawerContent } from '../store/signals';
import { t } from '../i18n';

// ── 后端 /scenarios 契约（routes/scenarios.rs） ──────────────────────────────
interface ScenarioFormRef {
  plugin_id: string;
  form_id: string;
}
interface Scenario {
  plugin_id: string;
  plugin_label: string;
  plugin_type: string;
  agent_id: string;
  label: string;
  intent: string | null;
  scenario: string | null;
  cost_tier: 'free' | 'cloud';
  llm_required: boolean;
  case_kind: string | null;
  has_form: boolean;
  form_ref: ScenarioFormRef | null;
  output_modes: { default?: string | null; supports?: string[] } | null;
  enabled: boolean;
  /** EntitlementCache 运行态：free / active / trial / grace / trial-expired / revoked 等 */
  entitlement_status: string;
}
interface ScenariosResp {
  scenarios: Scenario[];
}

/** entitlement_status 是否锁定该卡（点击不 dispatch，显示购买引导）。 */
function isLocked(s: Scenario): boolean {
  return s.entitlement_status === 'trial-expired' || s.entitlement_status === 'revoked';
}

/** 通用输入（无 form 的 agent）→ POST /agents/{id}/run 的响应形态。 */
type AgentRunResponse = {
  ok?: boolean;
  output?: Record<string, unknown>;
  audit_trail?: string;
  red_lines_violated?: boolean;
};

export function WorkbenchView(): JSX.Element {
  const scenarios = useSignal<Scenario[] | null>(null);
  const loading = useSignal(true);
  const loadError = useSignal<string | null>(null);
  // 当前打开的表单卡片（has_form）
  const openForm = useSignal<Scenario | null>(null);
  // 当前打开的通用输入卡片（无 form）
  const openGeneric = useSignal<Scenario | null>(null);

  async function reload(): Promise<void> {
    loading.value = true;
    loadError.value = null;
    try {
      const resp = await api.get<ScenariosResp>('/scenarios');
      scenarios.value = resp.scenarios;
    } catch (e) {
      loadError.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  useEffect(() => {
    void reload();
  }, []);

  function onCardClick(s: Scenario): void {
    if (isLocked(s)) {
      toast('error', t('workbench.locked_hint'));
      return;
    }
    // 💰 卡需 LLM 但很可能未配 → 仍允许点（后端 exit 4 会回错）；
    // 这里仅做软提示路径由结果面板/错误处理兜底（spec §7 退出码 4）。
    if (s.has_form) {
      openForm.value = s;
    } else {
      openGeneric.value = s;
    }
  }

  if (loading.value) {
    return <Centered>{t('workbench.loading')}</Centered>;
  }
  if (loadError.value) {
    return (
      <Centered>
        <div style={{ color: 'var(--color-error)', marginBottom: 'var(--space-3)' }}>
          {t('workbench.load_failed')}: {loadError.value}
        </div>
        <Button variant="secondary" size="sm" onClick={() => void reload()}>
          {t('workbench.retry')}
        </Button>
      </Centered>
    );
  }

  const all = scenarios.value ?? [];
  if (all.length === 0) {
    return (
      <div style={{ padding: 'var(--space-6)' }}>
        <Header />
        <SuggestionsPanel />
        <EmptyState
          icon="🛠"
          title={t('workbench.empty_title')}
          description={t('workbench.empty_desc')}
          actions={[
            {
              label: t('workbench.empty_action'),
              onClick: () => (currentView.value = 'marketplace'),
              variant: 'primary',
            },
          ]}
        />
      </div>
    );
  }

  // 按 plugin 分组（保持插入顺序）
  const groups = new Map<string, { label: string; items: Scenario[] }>();
  for (const s of all) {
    const g = groups.get(s.plugin_id);
    if (g) g.items.push(s);
    else groups.set(s.plugin_id, { label: s.plugin_label || s.plugin_id, items: [s] });
  }

  return (
    <div style={{ padding: 'var(--space-6)', display: 'flex', flexDirection: 'column', gap: 'var(--space-5)' }}>
      <Header />
      <SuggestionsPanel />
      {[...groups.entries()].map(([pluginId, g]) => (
        <section key={pluginId}>
          <h3
            style={{
              fontSize: 'var(--text-base)',
              fontWeight: 600,
              margin: '0 0 var(--space-3) 0',
              color: 'var(--color-text)',
            }}
          >
            {g.label}
          </h3>
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(auto-fill, minmax(220px, 1fr))',
              gap: 'var(--space-3)',
            }}
          >
            {g.items.map((s) => (
              <ScenarioCard key={s.agent_id} scenario={s} onClick={() => onCardClick(s)} />
            ))}
          </div>
        </section>
      ))}

      {openForm.value && (
        <Modal open onClose={() => (openForm.value = null)} title={openForm.value.label}>
          <PluginForm
            pluginId={openForm.value.form_ref!.plugin_id}
            formId={openForm.value.form_ref!.form_id}
            onClose={() => (openForm.value = null)}
          />
        </Modal>
      )}

      {openGeneric.value && (
        <Modal open onClose={() => (openGeneric.value = null)} title={openGeneric.value.label}>
          <GenericRunForm scenario={openGeneric.value} onClose={() => (openGeneric.value = null)} />
        </Modal>
      )}
    </div>
  );
}

function Header(): JSX.Element {
  return (
    <div style={{ marginBottom: 'var(--space-2)' }}>
      <h2 style={{ margin: '0 0 var(--space-1) 0', fontSize: 'var(--text-2xl)', fontWeight: 600 }}>
        {t('workbench.title')}
      </h2>
      <p style={{ margin: 0, fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>
        {t('workbench.subtitle')}
      </p>
    </div>
  );
}

function Centered({ children }: { children: preact.ComponentChildren }): JSX.Element {
  return (
    <div
      style={{
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 'var(--space-6)',
        color: 'var(--color-text-secondary)',
      }}
    >
      {children}
    </div>
  );
}

function ScenarioCard({ scenario: s, onClick }: { scenario: Scenario; onClick: () => void }): JSX.Element {
  const locked = isLocked(s);
  const costIcon = s.cost_tier === 'free' ? '🆓' : '💰';
  const costLabel = s.cost_tier === 'free' ? t('workbench.cost.free') : t('workbench.cost.cloud');
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-2)',
        padding: 'var(--space-3)',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        opacity: s.enabled ? 1 : 0.55,
      }}
    >
      <div style={{ fontSize: 'var(--text-sm)', fontWeight: 600, color: 'var(--color-text)' }}>
        {s.label}
      </div>
      <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', minHeight: 16 }}>
        {/* intent 是 plugin 声明的行业术语（language-neutral 例外，走 t() 包成本标识） */}
        {costIcon} {costLabel}
        {s.intent ? ` · ${s.intent}` : ''}
      </div>
      <div style={{ marginTop: 'auto' }}>
        {locked ? (
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              settingsInitialTab.value = 'member';
              currentView.value = 'settings';
            }}
          >
            🔒 {t('workbench.locked')}
          </Button>
        ) : (
          <Button variant="primary" size="sm" onClick={onClick} disabled={!s.enabled}>
            ▶ {t('workbench.run')}
          </Button>
        )}
      </div>
    </div>
  );
}

/** 无 form 的 agent：通用「贴文本」输入 → POST /agents/{id}/run。
 *  结果走 agent-result drawer（与 PluginForm 一致的展示路径）。 */
function GenericRunForm({ scenario: s, onClose }: { scenario: Scenario; onClose: () => void }): JSX.Element {
  const text = useSignal('');
  const submitting = useSignal(false);

  async function run(): Promise<void> {
    const body = text.value.trim();
    if (!body) {
      toast('error', t('pluginForm.required', { fields: 'text' }));
      return;
    }
    submitting.value = true;
    try {
      const resp = await api.post<AgentRunResponse>(`/agents/${encodeURIComponent(s.agent_id)}/run`, {
        input: { text: body },
      });
      const blocked = resp.ok === false || resp.red_lines_violated === true;
      drawerContent.value = {
        type: 'agent-result',
        result: {
          title: s.label,
          meta: s.scenario ?? '',
          facts: [],
          requiredCount: 0,
          blockedReason: blocked ? resp.audit_trail || t('pluginForm.blocked') : undefined,
        },
      };
      onClose();
    } catch (e) {
      toast('error', t('pluginForm.runError', { msg: e instanceof Error ? e.message : String(e) }));
    } finally {
      submitting.value = false;
    }
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)', minWidth: 360 }}>
      {s.scenario && (
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: 0 }}>
          {s.scenario}
        </p>
      )}
      <textarea
        value={text.value}
        onInput={(e) => (text.value = (e.currentTarget as HTMLTextAreaElement).value)}
        rows={6}
        placeholder={t('workbench.subtitle')}
        style={{
          width: '100%',
          fontSize: 'var(--text-sm)',
          padding: 'var(--space-2)',
          border: '1px solid var(--color-border)',
          borderRadius: 'var(--radius-sm)',
          resize: 'vertical',
        }}
      />
      <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
        <Button variant="secondary" size="sm" onClick={onClose} disabled={submitting.value}>
          {t('common.cancel')}
        </Button>
        <Button variant="primary" size="sm" onClick={() => void run()} loading={submitting.value} disabled={submitting.value}>
          ▶ {t('workbench.run')}
        </Button>
      </div>
    </div>
  );
}
