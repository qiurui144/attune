/** Skills 视图 · plugin 注册的所有 type=skill · toggle 启用/禁用 */
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState } from '../components';
import { toast } from '../components/Toast';
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
      toast('error', '保存失败');
      return;
    }
    skills.value = skills.value.map((x) =>
      x.id === s.id ? { ...x, disabled_by_user: !enable } : x
    );
    toast('success', enable ? '已启用' : '已禁用');
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
          🧠 Skills
        </h2>
        <Button variant="secondary" size="sm" onClick={() => void refresh()}>
          刷新
        </Button>
      </header>

      <p style={{ color: 'var(--color-text-secondary)', margin: 0, fontSize: 'var(--text-sm)' }}>
        chat 关键词触发到你安装的 skill。免费版和 Pro 版机制相同；自己写或下载 .attunepkg
        解压到 <code>~/.local/share/attune/plugins/&lt;name&gt;/</code> 即可。
      </p>

      {loading.value ? (
        <div style={{ color: 'var(--color-text-secondary)' }}>加载中…</div>
      ) : skills.value.length === 0 ? (
        <EmptyState
          icon="🧠"
          title="还没安装任何技能"
          description="在插件市场可以浏览并安装社区分享的技能与智能助手。"
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
            <span style={{ color: 'var(--color-text-secondary)' }}>关键词触发：</span>
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
                + {s.patterns.length} 正则
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
            ? '插件未启用'
            : s.disabled_by_user
              ? '已禁用'
              : '已启用'}
        </span>
      </label>
    </div>
  );
}
