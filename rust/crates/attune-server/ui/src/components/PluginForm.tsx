/** PluginForm · 通用 plugin-form 渲染器
 *
 * 通用（非任何行业专属，遵守 OSS 边界）：拉 plugin 声明的 FormSchema →
 * 渲染字段 → 提交时按点分字段名展开成嵌套 JSON → 调 /agents/{id}/run →
 * agent 输出映射为变体 A AgentResult → 打开 agent-result drawer。
 *
 * 行业字段 / 标签 / 嵌套结构全部来自 plugin 的 forms/<id>.yaml，本组件零行业耦合。
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button } from './Button';
import { toast } from './Toast';
import { api } from '../store/api';
import { drawerContent } from '../store/signals';
import { t } from '../i18n';
import type { AgentResult, AgentFact } from './AgentResultPanel';

/** plugin 声明的表单字段（对齐 attune-core::ui_runtime::FormField 的 JSON 形态）。 */
type FormField = {
  name: string;
  label: string;
  type: string; // text | number | date | select | textarea | checkbox
  required?: boolean;
  placeholder?: string;
  options?: { value: string; label: string }[];
  help?: string;
  default_value?: string | null;
};

type FormSchema = {
  id: string;
  title: string;
  description?: string;
  fields: FormField[];
  submit_target: string; // "agent:<agent_id>"
};

type AgentRunResponse = {
  ok?: boolean;
  output?: Record<string, unknown>;
  audit_trail?: string;
};

export type PluginFormProps = {
  pluginId: string;
  formId: string;
  onClose: () => void;
};

/** 点分字段名 → 嵌套对象写入：setNested(o,'facts.parties.name',v)。 */
function setNested(root: Record<string, unknown>, path: string, value: unknown): void {
  const keys = path.split('.');
  let node = root;
  for (let i = 0; i < keys.length - 1; i++) {
    const k = keys[i];
    if (typeof node[k] !== 'object' || node[k] === null) node[k] = {};
    node = node[k] as Record<string, unknown>;
  }
  node[keys[keys.length - 1]] = value;
}

/** snake_case → 首字母大写空格分隔，给 computation 行做可读 label。 */
function prettify(key: string): string {
  const s = key.replace(/_/g, ' ').trim();
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function fmtNum(n: number): string {
  return Number.isInteger(n) ? String(n) : n.toFixed(2);
}

/** agent 输出 → 变体 A AgentResult（通用 best-effort 映射）。 */
function toAgentResult(
  schema: FormSchema,
  values: Record<string, string>,
  resp: AgentRunResponse,
): AgentResult {
  const out = resp.output ?? {};
  const comp = out.computation as Record<string, unknown> | undefined;
  const redLines = (out.red_lines_violated as string[] | undefined) ?? [];
  const blocked = resp.ok === false || redLines.length > 0;

  const facts: AgentFact[] = schema.fields.map((f) => {
    const v = values[f.name];
    const has = v !== undefined && v !== '' && v !== null;
    return {
      field: f.name,
      label: f.label,
      value: has ? v : null,
      citations: [],
      source: 'lawyer' as const,
      verified: has,
    };
  });

  let computation: AgentResult['computation'];
  if (!blocked && comp) {
    const rows = Object.entries(comp)
      .filter(([, v]) => typeof v === 'number')
      .map(([k, v]) => ({ label: prettify(k), value: fmtNum(v as number) }));
    computation = { formula: String(comp.formula_used ?? ''), rows };
  }

  return {
    title: schema.title,
    meta: comp?.formula_used ? String(comp.formula_used) : '',
    facts,
    requiredCount: schema.fields.filter((f) => f.required).length,
    computation,
    blockedReason: blocked
      ? redLines.join('；') || resp.audit_trail || t('pluginForm.blocked')
      : undefined,
  };
}

export function PluginForm({ pluginId, formId, onClose }: PluginFormProps): JSX.Element {
  const schema = useSignal<FormSchema | null>(null);
  const values = useSignal<Record<string, string>>({});
  const loading = useSignal(true);
  const loadError = useSignal<string | null>(null);
  const submitting = useSignal(false);

  useEffect(() => {
    loading.value = true;
    loadError.value = null;
    void (async () => {
      try {
        const s = await api.get<FormSchema>(
          `/forms/${encodeURIComponent(pluginId)}/${encodeURIComponent(formId)}/schema`,
        );
        schema.value = s;
        // 用 default_value 初始化
        const init: Record<string, string> = {};
        for (const f of s.fields) {
          if (f.default_value != null) init[f.name] = f.default_value;
        }
        values.value = init;
      } catch (e) {
        loadError.value = e instanceof Error ? e.message : String(e);
      } finally {
        loading.value = false;
      }
    })();
  }, [pluginId, formId]);

  function setValue(name: string, v: string): void {
    values.value = { ...values.value, [name]: v };
  }

  async function submit(): Promise<void> {
    const s = schema.value;
    if (!s) return;
    // 必填校验
    const missing = s.fields.filter(
      (f) => f.required && f.type !== 'checkbox' && !(values.value[f.name] ?? '').trim(),
    );
    if (missing.length > 0) {
      toast('error', t('pluginForm.required', { fields: missing.map((f) => f.label).join('、') }));
      return;
    }
    // 字段值 → 按 type 转 JSON 值 → 点分名展开为嵌套 input
    const input: Record<string, unknown> = {};
    for (const f of s.fields) {
      const raw = values.value[f.name];
      if (f.type === 'checkbox') {
        setNested(input, f.name, raw === 'true');
      } else if (raw === undefined || raw === '') {
        continue; // 空的可选字段 → 不传，由后端 serde default 处理
      } else if (f.type === 'number') {
        const n = Number(raw);
        setNested(input, f.name, Number.isNaN(n) ? raw : n);
      } else {
        setNested(input, f.name, raw);
      }
    }
    // submit_target "agent:<id>" → agent id
    const agentId = s.submit_target.startsWith('agent:')
      ? s.submit_target.slice('agent:'.length)
      : s.submit_target;

    submitting.value = true;
    try {
      const resp = await api.post<AgentRunResponse>(
        `/agents/${encodeURIComponent(agentId)}/run`,
        { input },
      );
      drawerContent.value = { type: 'agent-result', result: toAgentResult(s, values.value, resp) };
      onClose();
    } catch (e) {
      toast('error', t('pluginForm.runError', { msg: e instanceof Error ? e.message : String(e) }));
    } finally {
      submitting.value = false;
    }
  }

  if (loading.value) {
    return <div style={{ color: 'var(--color-text-secondary)' }}>{t('common.loading')}</div>;
  }
  if (loadError.value || !schema.value) {
    return (
      <div style={{ color: 'var(--color-error)' }}>
        {t('pluginForm.loadError', { msg: loadError.value ?? '' })}
      </div>
    );
  }
  const s = schema.value;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)', minWidth: 360 }}>
      {s.description && (
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: 0 }}>
          {s.description}
        </p>
      )}
      {s.fields.length === 0 && (
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-disabled)' }}>
          {t('pluginForm.noFields')}
        </p>
      )}
      {s.fields.map((f) => (
        <FieldRow key={f.name} field={f} value={values.value[f.name] ?? ''} onChange={(v) => setValue(f.name, v)} />
      ))}
      <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)', marginTop: 'var(--space-2)' }}>
        <Button variant="secondary" size="sm" onClick={onClose} disabled={submitting.value}>
          {t('common.cancel')}
        </Button>
        <Button variant="primary" size="sm" onClick={() => void submit()} loading={submitting.value} disabled={submitting.value}>
          {submitting.value ? t('pluginForm.submitting') : t('pluginForm.submit')}
        </Button>
      </div>
    </div>
  );
}

function FieldRow({
  field: f,
  value,
  onChange,
}: {
  field: FormField;
  value: string;
  onChange: (v: string) => void;
}): JSX.Element {
  const labelEl = (
    <span style={{ fontSize: 'var(--text-sm)', fontWeight: 500 }}>
      {f.label}
      {f.required && <span style={{ color: 'var(--color-error)' }}> *</span>}
    </span>
  );
  const inputStyle = {
    width: '100%',
    fontSize: 'var(--text-sm)',
    padding: '6px 8px',
    border: '1px solid var(--color-border)',
    borderRadius: 'var(--radius-sm)',
  };

  let control: JSX.Element;
  if (f.type === 'select') {
    control = (
      <select value={value} onChange={(e) => onChange(e.currentTarget.value)} style={inputStyle}>
        <option value="">—</option>
        {(f.options ?? []).map((o) => (
          <option key={o.value} value={o.value}>{o.label}</option>
        ))}
      </select>
    );
  } else if (f.type === 'checkbox') {
    control = (
      <label style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)', cursor: 'pointer' }}>
        <input
          type="checkbox"
          checked={value === 'true'}
          onChange={(e) => onChange((e.currentTarget as HTMLInputElement).checked ? 'true' : 'false')}
        />
        <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>{f.help}</span>
      </label>
    );
  } else if (f.type === 'textarea') {
    control = (
      <textarea
        value={value}
        placeholder={f.placeholder}
        onInput={(e) => onChange((e.currentTarget as HTMLTextAreaElement).value)}
        rows={3}
        style={inputStyle}
      />
    );
  } else {
    control = (
      <input
        type={f.type === 'number' ? 'number' : f.type === 'date' ? 'date' : 'text'}
        value={value}
        placeholder={f.placeholder}
        onInput={(e) => onChange((e.currentTarget as HTMLInputElement).value)}
        style={inputStyle}
      />
    );
  }

  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      {labelEl}
      {control}
      {f.help && f.type !== 'checkbox' && (
        <span style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-disabled)' }}>{f.help}</span>
      )}
    </label>
  );
}
