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
import { items } from '../store/signals';
import { loadItems } from '../hooks/useItems';
import {
  listSkills,
  estimateSkill,
  dryRunSkill,
  runSkill,
  listSkillVersions,
  captureSkillSnapshot,
  activateSkillSnapshot,
  clearSkillSnapshot,
  SkillError,
  type SkillInfo,
  type SkillEstimate,
  type SkillDryRunResult,
  type SkillVersionEntry,
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
  const dryRun = useSignal<SkillDryRunResult | null>(null);
  const running = useSignal(false);
  const dryRunning = useSignal(false);
  const versions = useSignal<SkillVersionEntry[]>([]);
  const versionBusy = useSignal(false);
  const lastResult = useSignal<{ filename: string; warnings: string[]; partial: boolean } | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        skills.value = await listSkills();
        if (skills.value.length > 0) selectedId.value = skills.value[0].id;
        versions.value = await listSkillVersions().catch(() => []);
        if (skills.value.some((s) => s.inputs.some((input) => input.ty === 'itemid'))) {
          await loadItems(100, 0);
        }
      } catch {
        toast('error', t('skillRunner.error.list'));
      } finally {
        loading.value = false;
      }
    })();
  }, []);

  const selected = skills.value.find((s) => s.id === selectedId.value) ?? null;
  const selectedVersion = versions.value.find((v) => v.skill_id === selectedId.value) ?? null;
  const canRun = Boolean(estimate.value && !estimate.value.over_cap && !running.value);

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
      dryRun.value = null;
    } catch {
      toast('error', t('skillRunner.error.estimate'));
    }
  }

  async function doDryRun(): Promise<void> {
    if (!selected) return;
    dryRunning.value = true;
    try {
      const res = await dryRunSkill(selected.id, buildInputs(selected));
      dryRun.value = res;
      estimate.value = res.estimate;
      toast(res.can_run ? 'success' : 'error', res.can_run ? t('skillRunner.dry.ok') : t('skillRunner.dry.blocked'));
    } catch {
      toast('error', t('skillRunner.error.dryRun'));
    } finally {
      dryRunning.value = false;
    }
  }

  async function refreshVersions(): Promise<void> {
    versions.value = await listSkillVersions().catch(() => versions.value);
  }

  async function captureCurrent(activate: boolean): Promise<void> {
    if (!selected) return;
    versionBusy.value = true;
    try {
      const updated = await captureSkillSnapshot(selected.id, '', activate);
      versions.value = versions.value.filter((entry) => entry.skill_id !== selected.id).concat(updated);
      toast('success', activate ? t('skillRunner.version.savedActivated') : t('skillRunner.version.saved'));
      await refreshVersions();
    } catch {
      toast('error', t('skillRunner.version.saveFailed'));
    } finally {
      versionBusy.value = false;
    }
  }

  async function activateSnapshot(hash: string): Promise<void> {
    if (!selected) return;
    versionBusy.value = true;
    try {
      const updated = await activateSkillSnapshot(selected.id, hash);
      versions.value = versions.value.filter((entry) => entry.skill_id !== selected.id).concat(updated);
      toast('success', t('skillRunner.version.activated'));
      await refreshVersions();
    } catch {
      toast('error', t('skillRunner.version.activateFailed'));
    } finally {
      versionBusy.value = false;
    }
  }

  async function clearActiveSnapshot(): Promise<void> {
    if (!selected) return;
    versionBusy.value = true;
    try {
      const updated = await clearSkillSnapshot(selected.id);
      versions.value = versions.value.filter((entry) => entry.skill_id !== selected.id).concat(updated);
      toast('success', t('skillRunner.version.cleared'));
      await refreshVersions();
    } catch {
      toast('error', t('skillRunner.version.clearFailed'));
    } finally {
      versionBusy.value = false;
    }
  }

  async function doRun(): Promise<void> {
    if (!selected) return;
    if (!estimate.value) {
      toast('error', t('skillRunner.error.estimateRequired'));
      return;
    }
    if (estimate.value.over_cap) {
      toast('error', t('skillRunner.cost.overCap'));
      return;
    }
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
                dryRun.value = null;
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
                {' · '}
                {t('skillRunner.version', { version: selected.version })}
              </p>

              {selected.inputs.map((spec) => (
                <div key={spec.name} style={labelStyle}>
                  <span>
                    {spec.name}
                    {spec.required ? ' *' : ''}
                    {spec.ty === 'stringlist' ? ` — ${t('skillRunner.hint.commaList')}` : ''}
                  </span>
                  {spec.ty === 'itemid' ? (
                    <select
                      data-testid={`skillrunner-input-${spec.name}`}
                      value={inputs.value[spec.name] ?? ''}
                      onChange={(e) => {
                        inputs.value = { ...inputs.value, [spec.name]: (e.target as HTMLSelectElement).value };
                        estimate.value = null;
                        dryRun.value = null;
                      }}
                      style={selectStyle}
                    >
                      <option value="">{t('skillRunner.placeholder.itemId')}</option>
                      {items.value.map((item) => (
                        <option key={item.id} value={item.id}>
                          {item.title || item.id}
                        </option>
                      ))}
                    </select>
                  ) : (
                    <input
                      type="text"
                      data-testid={`skillrunner-input-${spec.name}`}
                      value={inputs.value[spec.name] ?? ''}
                      placeholder={spec.ty === 'stringlist' ? t('skillRunner.placeholder.entities') : ''}
                      onInput={(e) => {
                        inputs.value = { ...inputs.value, [spec.name]: (e.target as HTMLInputElement).value };
                        estimate.value = null;
                        dryRun.value = null;
                      }}
                      style={inputStyle}
                    />
                  )}
                </div>
              ))}

              <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'center', flexWrap: 'wrap' }}>
                <Button variant="secondary" size="sm" onClick={() => void doEstimate()}>
                  {t('skillRunner.action.estimate')}
                </Button>
                <Button variant="secondary" size="sm" disabled={dryRunning.value} onClick={() => void doDryRun()}>
                  {dryRunning.value ? t('skillRunner.dry.running') : t('skillRunner.action.dryRun')}
                </Button>
                <Button variant="primary" size="sm" disabled={!canRun}
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
                {!estimate.value && (
                  <span style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)' }}>
                    {t('skillRunner.cost.estimateFirst')}
                  </span>
                )}
              </div>

              <div data-testid="skillrunner-version-control" style={resultCardStyle}>
                <div style={{ display: 'flex', justifyContent: 'space-between', gap: 'var(--space-3)', flexWrap: 'wrap' }}>
                  <div>
                    <div style={{ fontSize: 'var(--text-sm)', fontWeight: 600 }}>
                      {t('skillRunner.versionControl.title')}
                    </div>
                    <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', marginTop: 2 }}>
                      {selectedVersion
                        ? t('skillRunner.versionControl.current', {
                            version: selectedVersion.current.version,
                            hash: selectedVersion.current.hash.slice(0, 8),
                          })
                        : t('skillRunner.versionControl.unavailable')}
                    </div>
                    {selectedVersion?.active && (
                      <div
                        style={{
                          fontSize: 'var(--text-xs)',
                          color: selectedVersion.drift ? 'var(--color-warning)' : 'var(--color-success)',
                          marginTop: 2,
                        }}
                      >
                        {t('skillRunner.versionControl.active', {
                          version: selectedVersion.active.version,
                          hash: selectedVersion.active.hash.slice(0, 8),
                        })}
                        {selectedVersion.drift ? ` · ${t('skillRunner.versionControl.drift')}` : ''}
                      </div>
                    )}
                  </div>
                  <div style={{ display: 'flex', gap: 'var(--space-2)', flexWrap: 'wrap' }}>
                    <Button size="sm" variant="secondary" disabled={versionBusy.value} onClick={() => void captureCurrent(false)}>
                      {t('skillRunner.versionControl.snapshot')}
                    </Button>
                    <Button size="sm" variant="secondary" disabled={versionBusy.value} onClick={() => void captureCurrent(true)}>
                      {t('skillRunner.versionControl.snapshotActivate')}
                    </Button>
                    {selectedVersion?.active && (
                      <Button size="sm" variant="ghost" disabled={versionBusy.value} onClick={() => void clearActiveSnapshot()}>
                        {t('skillRunner.versionControl.clear')}
                      </Button>
                    )}
                  </div>
                </div>
                {selectedVersion && selectedVersion.history.length > 0 && (
                  <div style={{ marginTop: 'var(--space-3)', display: 'grid', gap: 'var(--space-2)' }}>
                    {selectedVersion.history.slice(0, 5).map((snap) => (
                      <div
                        key={snap.hash}
                        style={{
                          display: 'grid',
                          gridTemplateColumns: 'minmax(0, 1fr) auto',
                          gap: 'var(--space-2)',
                          alignItems: 'center',
                          fontSize: 'var(--text-xs)',
                          color: 'var(--color-text-secondary)',
                        }}
                      >
                        <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                          v{snap.version} · {snap.hash.slice(0, 8)} · {new Date(snap.captured_at).toLocaleString()}
                        </span>
                        <Button size="sm" variant="ghost" disabled={versionBusy.value} onClick={() => void activateSnapshot(snap.hash)}>
                          {t('skillRunner.versionControl.activate')}
                        </Button>
                      </div>
                    ))}
                  </div>
                )}
              </div>

              {estimate.value && (
                <div data-testid="skillrunner-estimate" style={resultCardStyle}>
                  <div style={{ display: 'flex', justifyContent: 'space-between', gap: 'var(--space-3)', flexWrap: 'wrap' }}>
                    <div style={{ fontSize: 'var(--text-sm)', fontWeight: 600 }}>
                      {t('skillRunner.cost.summary')}
                    </div>
                    <div
                      style={{
                        fontSize: 'var(--text-xs)',
                        color: estimate.value.over_cap ? 'var(--color-danger)' : 'var(--color-text-secondary)',
                      }}
                    >
                      {t('skillRunner.cost.chip', {
                        tokens: Math.round(estimate.value.est_tokens / 100) / 10,
                        usd: estimate.value.est_usd.toFixed(4),
                        seconds: estimate.value.est_seconds,
                      })}
                    </div>
                  </div>
                  {estimate.value.steps.length > 0 && (
                    <div style={{ marginTop: 'var(--space-3)', display: 'grid', gap: 'var(--space-2)' }}>
                      {estimate.value.steps.map((step) => (
                        <div
                          key={step.id}
                          style={{
                            display: 'grid',
                            gridTemplateColumns: 'minmax(0, 1fr) auto',
                            gap: 'var(--space-3)',
                            alignItems: 'center',
                            fontSize: 'var(--text-xs)',
                            color: 'var(--color-text-secondary)',
                          }}
                        >
                          <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                            {step.id}
                          </span>
                          <span>
                            {t('skillRunner.cost.step', {
                              tier: step.tier,
                              tokens: Math.round(step.est_tokens / 100) / 10,
                            })}
                          </span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}

              {dryRun.value && (
                <div data-testid="skillrunner-dry-run" style={resultCardStyle}>
                  <div
                    style={{
                      fontSize: 'var(--text-sm)',
                      fontWeight: 600,
                      color: dryRun.value.can_run ? 'var(--color-success)' : 'var(--color-error)',
                    }}
                  >
                    {dryRun.value.can_run ? t('skillRunner.dry.canRun') : t('skillRunner.dry.cannotRun')}
                  </div>
                  {dryRun.value.blockers.length > 0 && (
                    <div style={{ marginTop: 'var(--space-2)', display: 'grid', gap: 'var(--space-1)' }}>
                      {dryRun.value.blockers.map((blocker) => (
                        <div key={blocker} style={{ fontSize: 'var(--text-xs)', color: 'var(--color-error)' }}>
                          {blocker}
                        </div>
                      ))}
                    </div>
                  )}
                  {dryRun.value.warnings.length > 0 && (
                    <div style={{ marginTop: 'var(--space-2)', display: 'grid', gap: 'var(--space-1)' }}>
                      {dryRun.value.warnings.map((warning) => (
                        <div key={warning} style={{ fontSize: 'var(--text-xs)', color: 'var(--color-warning)' }}>
                          {warning}
                        </div>
                      ))}
                    </div>
                  )}
                  <div style={{ marginTop: 'var(--space-3)', display: 'grid', gap: 'var(--space-2)' }}>
                    {dryRun.value.steps.map((step) => (
                      <div
                        key={step.id}
                        style={{
                          display: 'grid',
                          gridTemplateColumns: 'minmax(0, 1fr) auto',
                          gap: 'var(--space-2)',
                          fontSize: 'var(--text-xs)',
                          color: 'var(--color-text-secondary)',
                        }}
                      >
                        <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{step.id}</span>
                        <span>{step.kind} · {step.tier}{step.detail ? ` · ${step.detail}` : ''}</span>
                      </div>
                    ))}
                  </div>
                  {dryRun.value.referenced_items.length > 0 && (
                    <div style={{ marginTop: 'var(--space-3)', display: 'grid', gap: 'var(--space-1)' }}>
                      {dryRun.value.referenced_items.map((item) => (
                        <div key={item.id} style={{ fontSize: 'var(--text-xs)', color: item.found ? 'var(--color-text-secondary)' : 'var(--color-error)' }}>
                          {item.found
                            ? t('skillRunner.dry.item', { title: item.title || item.id, chars: item.chars })
                            : t('skillRunner.dry.itemMissing', { id: item.id })}
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}

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
