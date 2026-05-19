/** Skills 视图 · plugin 注册的所有 type=skill · toggle 启用/禁用 */
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import {
  listSkills,
  setSkillDisabled,
  type SkillSummary,
} from '../hooks/useSkills';

export function SkillsView(): JSX.Element {
  const skills = useSignal<SkillSummary[]>([]);
  const loading = useSignal(true);

  async function refresh() {
    loading.value = true;
    skills.value = await listSkills();
    loading.value = false;
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function handleToggle(s: SkillSummary, enable: boolean) {
    const ok = await setSkillDisabled(s.id, !enable);
    if (!ok) {
      toast('error', t('skills.toast.save_failed'));
      return;
    }
    skills.value = skills.value.map((x) =>
      x.id === s.id ? { ...x, disabled_by_user: !enable } : x
    );
    toast('success', enable ? t('skills.toast.enabled') : t('skills.toast.disabled'));
  }

  return (
    <div
      style={{
        padding: 'var(--space-5)',
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-4)',
      }}
    >
      <header
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
        }}
      >
        <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0 }}>
          {t('skills.title')}
        </h2>
        <Button variant="secondary" size="sm" onClick={() => void refresh()}>
          {t('skills.refresh')}
        </Button>
      </header>

      <p style={{ color: 'var(--color-text-secondary)', margin: 0, fontSize: 'var(--text-sm)' }}>
        {t('skills.intro.before')}
        <code>~/.local/share/attune/plugins/&lt;name&gt;/</code>
        {t('skills.intro.after')}
      </p>

      {loading.value ? (
        <div style={{ color: 'var(--color-text-secondary)' }}>{t('common.loading')}</div>
      ) : skills.value.length === 0 ? (
        <EmptyState
          icon="🧠"
          title={t('skills.empty.title')}
          description={t('skills.empty.desc')}
        />
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
          {skills.value.map((s) => (
            <SkillRow key={s.id} skill={s} onToggle={(enable) => void handleToggle(s, enable)} />
          ))}
        </div>
      )}
    </div>
  );
}

function SkillRow({
  skill: s,
  onToggle,
}: {
  skill: SkillSummary;
  onToggle: (enable: boolean) => void;
}): JSX.Element {
  const checked = !s.disabled_by_user && s.enabled_in_plugin;
  const lockedOff = !s.enabled_in_plugin;
  return (
    <div
      style={{
        padding: 'var(--space-3) var(--space-4)',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        display: 'flex',
        gap: 'var(--space-3)',
        alignItems: 'flex-start',
      }}
    >
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--space-2)' }}>
          <strong style={{ fontSize: 'var(--text-base)' }}>{s.name}</strong>
          <span style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-xs)' }}>
            v{s.version}
          </span>
        </div>
        <div style={{
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-secondary)',
          fontFamily: 'var(--font-mono)',
        }}>
          {s.id}
        </div>
        {s.description && (
          <div style={{ marginTop: 'var(--space-2)', fontSize: 'var(--text-sm)' }}>
            {s.description}
          </div>
        )}
        {(s.keywords.length > 0 || s.patterns.length > 0) && (
          <div style={{
            marginTop: 'var(--space-2)',
            display: 'flex',
            flexWrap: 'wrap',
            gap: 'var(--space-1)',
            alignItems: 'center',
            fontSize: 'var(--text-xs)',
          }}>
            <span style={{ color: 'var(--color-text-secondary)' }}>{t('skills.keyword_trigger')}</span>
            {s.keywords.map((k) => (
              <span key={k} style={{
                background: 'var(--color-surface-hover)',
                padding: '1px 6px',
                borderRadius: 3,
              }}>{k}</span>
            ))}
            {s.patterns.length > 0 && (
              <span
                title={s.patterns.join('\n')}
                style={{ color: 'var(--color-text-secondary)', fontStyle: 'italic' }}
              >
                {t('skills.patterns_count', { count: s.patterns.length })}
              </span>
            )}
          </div>
        )}
      </div>
      <label style={{
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-2)',
        cursor: lockedOff ? 'not-allowed' : 'pointer',
        opacity: lockedOff ? 0.5 : 1,
      }}>
        <input
          type="checkbox"
          checked={checked}
          disabled={lockedOff}
          onChange={(e) => onToggle((e.target as HTMLInputElement).checked)}
        />
        <span style={{ fontSize: 'var(--text-sm)' }}>
          {lockedOff
            ? t('skills.status.plugin_disabled')
            : s.disabled_by_user
              ? t('skills.status.disabled')
              : t('skills.status.enabled')}
        </span>
      </label>
    </div>
  );
}
