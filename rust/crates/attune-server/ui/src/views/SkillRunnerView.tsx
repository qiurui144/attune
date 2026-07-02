/** Skill Runner view — pick a declarative skill, fill its inputs, see the cost estimate,
 * then run it to download a deliverable (CAP-2; 用户例 3: 设备参数比对成表 → 下载 xlsx).
 *
 * Cost discipline (CLAUDE.md §Cost&Trigger Contract): a skill run is 💰 — we fetch a 🆓 static
 * estimate first and show a chip (`~预估 NK tok · $0.00X · ~Ns`); the run button explicitly
 * confirms the cost. Warnings + partial-failure are surfaced after a run.
 *
 * i18n (project §i18n): every user-visible string goes through t(); no hardcoded CJK literal.
 */

import type { JSX } from 'preact';
import { useSignal } from '@preact/signals';
import { useEffect } from 'preact/hooks';
import { Button } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import {
  listSkills,
  estimateSkill,
  runSkill,
  SkillError,
  type SkillInfo,
  type SkillEstimate,
} from '../hooks/useSkillRuntime';

// ── shared inline styles ──
const containerStyle: JSX.CSSProperties = {
  padding: 'var(--space-6)',
  maxWidth: 760,
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
  ...inputStyle,
};

const labelStyle: JSX.CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  gap: 'var(--space-1)',
  fontSize: 'var(--text-sm)',
  color: 'var(--color-text-secondary)',
};

const resultCardStyle: JSX.CSSProperties = {
  marginTop: 'var(--space-4)',
  padding: 'var(--space-3)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  background: 'var(--color-surface)',
};

export function SkillRunnerView(): JSX.Element {
  const skills = useSignal<SkillInfo[]>([]);
  const loading = useSignal(true);
  const selectedId = useSignal<string | null>(null);
  const inputs = useSignal<Record<string, string>>({});
  const estimate = useSignal<SkillEstimate | null>(null);
  const running = useSignal(false);
  const lastResult = useSignal<{ filename: string; warnings: string[]; partial: boolean } | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        skills.value = await listSkills();
        if (skills.value.length > 0) selectedId.value = skills.value[0].id;
      } catch {
        toast('error', t('skillRunner.error.list'));
      } finally {
        loading.value = false;
      }
    })();
  }, []);

  const selected = skills.value.find((s) => s.id === selectedId.value) ?? null;

  function buildInputs(skill: SkillInfo): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    for (const spec of skill.inputs) {
      const raw = (inputs.value[spec.name] ?? '').trim();
      if (raw === '') continue;
      if (spec.ty === 'stringlist') {
        out[spec.name] = raw.split(',').map((s) => s.trim()).filter((s) => s !== '');
      } else {
        out[spec.name] = raw;
      }
    }
    return out;
  }

  async function doEstimate(): Promise<void> {
    if (!selected) return;
    try {
      estimate.value = await estimateSkill(selected.id, buildInputs(selected));
    } catch {
      toast('error', t('skillRunner.error.estimate'));
    }
  }

  async function doRun(): Promise<void> {
    if (!selected) return;
    running.value = true;
    lastResult.value = null;
    try {
      const res = await runSkill(selected.id, buildInputs(selected));
      lastResult.value = { filename: res.filename, warnings: res.warnings, partial: res.partial };
      toast('success', t('skillRunner.run.success', { filename: res.filename }));
    } catch (e) {
      const code = e instanceof SkillError ? e.code : 'skill-failed';
      const msg =
        code === 'member-required'
          ? t('skillRunner.error.member')
          : code === 'cloud-llm-disabled'
            ? t('skillRunner.error.egress')
            : code === 'input-invalid'
              ? t('skillRunner.error.input')
              : t('skillRunner.error.run');
      toast('error', msg);
    } finally {
      running.value = false;
    }
  }

  if (loading.value) {
    return <div style={containerStyle}>{t('skillRunner.loading')}</div>;
  }

  return (
    <div style={containerStyle}>
      <header style={{ marginBottom: 'var(--space-4)' }}>
        <h2 style={{ fontSize: 'var(--text-2xl)', fontWeight: 600, margin: 0 }}>
          {t('skillRunner.title')}
        </h2>
        <p style={{ color: 'var(--color-text-muted)', marginTop: 'var(--space-2)', fontSize: 'var(--text-sm)' }}>
          {t('skillRunner.subtitle')}
        </p>
      </header>

      {skills.value.length === 0 ? (
        <div data-testid="skillrunner-empty" style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>
          {t('skillRunner.empty')}
        </div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
          <div style={labelStyle}>
            <span>{t('skillRunner.label.skill')}</span>
            <select
              data-testid="skillrunner-select"
              value={selectedId.value ?? ''}
              onChange={(e) => {
                selectedId.value = (e.target as HTMLSelectElement).value;
                estimate.value = null;
                lastResult.value = null;
              }}
              style={selectStyle}
            >
              {skills.value.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.title || s.id} {s.source !== 'oss' ? `(${s.source})` : ''}
                </option>
              ))}
            </select>
          </div>

          {selected && (
            <>
              <p style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-sm)', margin: 0 }}>
                {selected.description}
              </p>

              {selected.inputs.map((spec) => (
                <div key={spec.name} style={labelStyle}>
                  <span>
                    {spec.name}
                    {spec.required ? ' *' : ''}
                    {spec.ty === 'stringlist' ? ` — ${t('skillRunner.hint.commaList')}` : ''}
                  </span>
                  <input
                    type="text"
                    data-testid={`skillrunner-input-${spec.name}`}
                    value={inputs.value[spec.name] ?? ''}
                    placeholder={
                      spec.ty === 'itemid'
                        ? t('skillRunner.placeholder.itemId')
                        : spec.ty === 'stringlist'
                          ? t('skillRunner.placeholder.entities')
                          : ''
                    }
                    onInput={(e) => {
                      inputs.value = { ...inputs.value, [spec.name]: (e.target as HTMLInputElement).value };
                      estimate.value = null;
                    }}
                    style={inputStyle}
                  />
                </div>
              ))}

              <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'center' }}>
                <Button variant="secondary" size="sm" onClick={() => void doEstimate()}>
                  {t('skillRunner.action.estimate')}
                </Button>
                <Button variant="primary" size="sm" disabled={running.value}
                  data-testid="skillrunner-run" onClick={() => void doRun()}>
                  {running.value ? t('skillRunner.running') : t('skillRunner.action.run')}
                </Button>
                {estimate.value && (
                  <span
                    data-testid="skillrunner-cost-chip"
                    title={t('skillRunner.cost.title')}
                    style={{
                      fontSize: 'var(--text-xs)',
                      color: estimate.value.over_cap ? 'var(--color-danger)' : 'var(--color-text-secondary)',
                      padding: 'var(--space-1) var(--space-2)',
                      background: 'var(--color-surface)',
                      border: '1px solid var(--color-border)',
                      borderRadius: 'var(--radius-md)',
                    }}
                  >
                    {t('skillRunner.cost.chip', {
                      tokens: Math.round(estimate.value.est_tokens / 100) / 10,
                      usd: estimate.value.est_usd.toFixed(4),
                      seconds: estimate.value.est_seconds,
                    })}
                    {estimate.value.over_cap ? ` · ${t('skillRunner.cost.overCap')}` : ''}
                  </span>
                )}
              </div>

              {lastResult.value && (
                <div data-testid="skillrunner-result" style={resultCardStyle}>
                  <div style={{ fontSize: 'var(--text-sm)' }}>
                    {t('skillRunner.result.downloaded', { filename: lastResult.value.filename })}
                  </div>
                  {lastResult.value.partial && (
                    <div style={{ color: 'var(--color-danger)', marginTop: 'var(--space-1)', fontSize: 'var(--text-sm)' }}>
                      {t('skillRunner.result.partial')}
                    </div>
                  )}
                  {lastResult.value.warnings.map((w, i) => (
                    <div key={i} style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-xs)', marginTop: 'var(--space-1)' }}>
                      ⚠ {w}
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
