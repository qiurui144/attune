/** Settings 视图 · Phase 6 · 真接 API（C 方案 左 tab + 右内容） */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal, useComputed } from '@preact/signals';
import { Button } from '../components';
import { toast } from '../components/Toast';
import {
  theme,
  vaultState,
  hardware,
  settings,
  memberState,
  settingsLocks,
  folderLinks,
} from '../store/signals';
import { setLocale, currentLocale, t } from '../i18n';
import { loadSettings, patchSettings } from '../hooks/useSettings';
import { loadMemberState, loadSettingsLocks, memberLogout, memberLoginPassword } from '../hooks/useMember';
import { loadFolderLinks } from '../hooks/useFolderLinks';
import { api, clearToken } from '../store/api';

/** LLM 厂商快捷预设 — 选中后自动填 endpoint + model，用户只需贴 API key。 */
type LlmPresetKey =
  | 'custom'
  | 'deepseek'
  | 'qwen'
  | 'glm'
  | 'kimi'
  | 'baichuan'
  | 'ollama'
  | 'openai';

interface LlmPreset {
  label: string;
  endpoint: string;
  model: string;
}

const LLM_PRESETS: Record<LlmPresetKey, LlmPreset> = {
  custom: { label: '自定义', endpoint: '', model: '' },
  deepseek: {
    label: 'DeepSeek (¥1/M tok, OpenAI 兼容)',
    endpoint: 'https://api.deepseek.com/v1',
    model: 'deepseek-chat',
  },
  qwen: {
    label: '阿里百炼 / Qwen (¥4/M tok)',
    endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    model: 'qwen-plus',
  },
  glm: {
    label: '智谱 GLM (¥50/M tok)',
    endpoint: 'https://open.bigmodel.cn/api/paas/v4',
    model: 'glm-4-plus',
  },
  kimi: {
    label: '月之暗面 Kimi (¥12/M tok)',
    endpoint: 'https://api.moonshot.cn/v1',
    model: 'moonshot-v1-8k',
  },
  baichuan: {
    label: '百川 (¥15/M tok)',
    endpoint: 'https://api.baichuan-ai.com/v1',
    model: 'Baichuan4-Turbo',
  },
  ollama: {
    label: 'Ollama 本地（免费）',
    endpoint: 'http://localhost:11434/v1',
    model: 'auto',
  },
  openai: {
    label: 'OpenAI (~¥3/M tok)',
    endpoint: 'https://api.openai.com/v1',
    model: 'gpt-4o-mini',
  },
};

type SettingsTab = 'general' | 'ai' | 'data' | 'member' | 'privacy' | 'about';

const TABS: Array<{ key: SettingsTab; icon: string; labelKey: string }> = [
  { key: 'general', icon: '⚙', labelKey: 'settings.tab.general' },
  { key: 'ai', icon: '🤖', labelKey: 'settings.tab.ai' },
  { key: 'data', icon: '📂', labelKey: 'settings.tab.data' },
  { key: 'member', icon: '👤', labelKey: 'settings.tab.member' },
  { key: 'privacy', icon: '🔐', labelKey: 'settings.tab.privacy' },
  { key: 'about', icon: 'ℹ', labelKey: 'settings.tab.about' },
];

export function SettingsView(): JSX.Element {
  const activeTab = useSignal<SettingsTab>('general');

  useEffect(() => {
    void loadSettings();
    // 同时刷新 hardware
    void api
      .get<Record<string, unknown>>('/status/diagnostics')
      .then((d) => (hardware.value = d))
      .catch(() => {});
  }, []);

  return (
    <div style={{ height: '100%', display: 'flex', minWidth: 0 }}>
      <nav
        aria-label="Settings sections"
        style={{
          width: 'clamp(170px, 22vw, 220px)',
          flexShrink: 0,
          borderRight: '1px solid var(--color-border)',
          padding: 'var(--space-5) 0',
          background: 'var(--color-bg)',
          overflow: 'auto',
        }}
      >
        <h2
          style={{
            fontSize: 'var(--text-xl)',
            fontWeight: 600,
            margin: 0,
            padding: '0 var(--space-5)',
            marginBottom: 'var(--space-4)',
          }}
        >
          {t('settings.title')}
        </h2>
        {TABS.map((tab) => {
          const active = activeTab.value === tab.key;
          return (
            <button
              key={tab.key}
              type="button"
              onClick={() => (activeTab.value = tab.key)}
              aria-current={active ? 'page' : undefined}
              className="interactive"
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 'var(--space-3)',
                width: '100%',
                padding: 'var(--space-2) var(--space-5)',
                background: active ? 'var(--color-surface-hover)' : 'transparent',
                borderLeft: `2px solid ${active ? 'var(--color-accent)' : 'transparent'}`,
                border: 'none',
                borderLeftWidth: 2,
                borderLeftStyle: 'solid',
                borderLeftColor: active ? 'var(--color-accent)' : 'transparent',
                color: active ? 'var(--color-text)' : 'var(--color-text-secondary)',
                fontSize: 'var(--text-sm)',
                textAlign: 'left',
                cursor: 'pointer',
              }}
            >
              <span aria-hidden="true">{tab.icon}</span>
              <span>{t(tab.labelKey)}</span>
            </button>
          );
        })}
      </nav>

      <div
        style={{
          flex: 1,
          minWidth: 0,
          overflow: 'auto',
          padding: 'clamp(var(--space-4), 3vw, var(--space-6)) clamp(var(--space-4), 4vw, var(--space-7))',
        }}
      >
        <div style={{ maxWidth: 980, width: '100%' }}>
          {activeTab.value === 'general' && <GeneralPanel />}
          {activeTab.value === 'ai' && <AIPanel />}
          {activeTab.value === 'data' && <DataPanel />}
          {activeTab.value === 'member' && <MemberPanel />}
          {activeTab.value === 'privacy' && <PrivacyPanel />}
          {activeTab.value === 'about' && <AboutPanel />}
        </div>
      </div>
    </div>
  );
}

function Section({
  title,
  desc,
  children,
}: {
  title: string;
  desc?: string;
  children?: JSX.Element | JSX.Element[] | (JSX.Element | false | null)[] | false | null;
}): JSX.Element {
  return (
    <section style={{ marginBottom: 'var(--space-6)' }}>
      <h3
        style={{
          fontSize: 'var(--text-lg)',
          fontWeight: 600,
          margin: 0,
          marginBottom: desc ? 'var(--space-1)' : 'var(--space-3)',
        }}
      >
        {title}
      </h3>
      {desc && (
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 var(--space-3) 0' }}>
          {desc}
        </p>
      )}
      <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
        {children}
      </div>
    </section>
  );
}

function GeneralPanel(): JSX.Element {
  return (
    <>
      <Section title={t('settings.section.appearance')}>
        <SettingRow label={t('settings.row.theme')}>
          <select
            value={theme.value}
            onChange={(e) => (theme.value = e.currentTarget.value as 'light' | 'dark' | 'auto')}
            style={selectStyle}
          >
            <option value="auto">{t('settings.theme.auto')}</option>
            <option value="light">{t('settings.theme.light')}</option>
            <option value="dark">{t('settings.theme.dark')}</option>
          </select>
        </SettingRow>
        <SettingRow label={t('settings.row.language')}>
          <select
            value={currentLocale.value}
            onChange={(e) => {
              setLocale(e.currentTarget.value as 'zh' | 'en');
              toast('success', t('settings.toast.lang_switched'));
            }}
            style={selectStyle}
          >
            <option value="zh">{t('settings.lang.zh')}</option>
            <option value="en">{t('settings.lang.en')}</option>
          </select>
        </SettingRow>
      </Section>
    </>
  );
}

function AIPanel(): JSX.Element {
  const llm = useComputed(() => (settings.value?.llm as Record<string, unknown>) ?? {});

  // 编辑态（草稿值，保存按钮才下发）
  const presetKey = useSignal<LlmPresetKey>('custom');
  const draftEndpoint = useSignal<string>('');
  const draftModel = useSignal<string>('');
  const draftApiKey = useSignal<string>('');
  const saving = useSignal(false);

  // 锁定联动 — Paid 会员锁 cloud_llm 字段
  useEffect(() => { void loadSettingsLocks(); }, []);
  const llmLocked = settingsLocks.value?.cloud_llm === 'locked';

  // 同步 server 值到草稿（首次加载 / 外部更新时）
  useEffect(() => {
    draftEndpoint.value = (llm.value.endpoint as string) ?? '';
    draftModel.value = (llm.value.model as string) ?? '';
  }, [llm.value.endpoint, llm.value.model]);

  const onPresetChange = (key: LlmPresetKey): void => {
    presetKey.value = key;
    if (key === 'custom') return; // 自定义：不动现有值
    const preset = LLM_PRESETS[key];
    draftEndpoint.value = preset.endpoint;
    draftModel.value = preset.model;
  };

  const onSave = async (): Promise<void> => {
    saving.value = true;
    try {
      const patch: Record<string, unknown> = {
        llm: {
          ...(settings.value?.llm as Record<string, unknown>),
          endpoint: draftEndpoint.value,
          model: draftModel.value,
        },
      };
      // 只有用户填了新 key 才下发（避免覆盖已有 key）
      if (draftApiKey.value.trim()) {
        (patch.llm as Record<string, unknown>).api_key = draftApiKey.value.trim();
      }
      const ok = await patchSettings(patch);
      if (ok) {
        draftApiKey.value = ''; // 清空输入框（key 已加密落盘）
        toast('success', t('settings.ai.llm.saved_toast'));
      } else {
        toast('error', t('settings.ai.llm.save_failed_toast'));
      }
    } finally {
      saving.value = false;
    }
  };

  const lockedInputStyle: Record<string, string | number> = llmLocked
    ? { ...(inputStyle as Record<string, string | number>), background: 'var(--color-surface-muted, #f4f5f7)', color: 'var(--color-text-secondary)', cursor: 'not-allowed' }
    : (inputStyle as Record<string, string | number>);

  return (
    <>
      <Section title={t('settings.ai.llm.title')}>
        {llmLocked && (
          <div style={{
            padding: 'var(--space-3)',
            background: 'rgba(212, 165, 116, 0.12)',
            border: '1px solid var(--color-warning)',
            borderRadius: 'var(--radius-md)',
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text)',
            marginBottom: 'var(--space-3)',
          }}>
            🔒 当前会员等级已锁定大模型配置（付费会员由云端 Gateway 自动下发，无需手动配置）
          </div>
        )}
        <SettingRow label={t('settings.ai.llm.preset')}>
          <select
            value={presetKey.value}
            disabled={llmLocked}
            onChange={(e) => onPresetChange(e.currentTarget.value as LlmPresetKey)}
            style={{ ...selectStyle, minWidth: 240, ...(llmLocked ? { background: 'var(--color-surface-muted, #f4f5f7)', cursor: 'not-allowed' } : {}) }}
            aria-label="LLM 厂商快捷预设"
          >
            {(Object.keys(LLM_PRESETS) as LlmPresetKey[]).map((k) => (
              <option key={k} value={k}>
                {LLM_PRESETS[k].label}
              </option>
            ))}
          </select>
        </SettingRow>
        <SettingRow label={t('settings.ai.llm.endpoint')}>
          <input
            type="text"
            value={draftEndpoint.value}
            disabled={llmLocked}
            onInput={(e) => (draftEndpoint.value = e.currentTarget.value)}
            placeholder="例：https://api.openai.com/v1"
            style={lockedInputStyle}
          />
        </SettingRow>
        <SettingRow label={t('settings.ai.llm.model')}>
          <input
            type="text"
            value={draftModel.value}
            disabled={llmLocked}
            onInput={(e) => (draftModel.value = e.currentTarget.value)}
            placeholder="例：deepseek-chat / qwen-plus / gpt-4o-mini"
            style={lockedInputStyle}
          />
        </SettingRow>
        <SettingRow label={t('settings.ai.llm.api_key')}>
          <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'center' }}>
            <input
              type="password"
              value={draftApiKey.value}
              disabled={llmLocked}
              onInput={(e) => (draftApiKey.value = e.currentTarget.value)}
              placeholder={llm.value.api_key_set ? t('settings.ai.llm.api_key_keep') : t('settings.ai.llm.api_key_placeholder')}
              style={lockedInputStyle}
            />
            <span
              style={{
                fontSize: 'var(--text-xs)',
                color: 'var(--color-text-secondary)',
                whiteSpace: 'nowrap',
              }}
            >
              {llm.value.api_key_set ? '●●●●●' : ''}
            </span>
          </div>
        </SettingRow>
        <SettingRow label="">
          <Button
            variant="primary"
            size="sm"
            onClick={() => void onSave()}
            disabled={saving.value || llmLocked}
          >
            {saving.value ? t('settings.ai.llm.saving') : t('settings.ai.llm.save')}
          </Button>
        </SettingRow>
      </Section>

      <Section title={t('settings.ai.web_search.title')}>
        <SettingRow label={t('settings.ai.web_search.enabled')}>
          <Toggle
            value={
              Boolean(
                (settings.value?.web_search as { enabled?: boolean })?.enabled,
              )
            }
            onChange={async (v) => {
              await patchSettings({
                web_search: {
                  ...(settings.value?.web_search as Record<string, unknown>),
                  enabled: v,
                },
              });
              toast('success', v ? t('settings.ai.web_search.enabled_toast') : t('settings.ai.web_search.disabled_toast'));
            }}
          />
        </SettingRow>
      </Section>
    </>
  );
}

function DataPanel(): JSX.Element {
  return (
    <>
      <Section title="数据源">
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: 0 }}>
          完整管理见左栏「远程目录」视图。
        </p>
      </Section>
      <FolderLinksSection />
      <Section title="备份与迁移">
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 8px 0' }}>
          导出后可用于换设备或备份。
        </p>
        <Button
          variant="secondary"
          size="sm"
          onClick={async () => {
            try {
              const res = await api.get<Record<string, unknown>>('/profile/export');
              const blob = new Blob([JSON.stringify(res, null, 2)], {
                type: 'application/json',
              });
              const url = URL.createObjectURL(blob);
              const a = document.createElement('a');
              a.href = url;
              a.download = `attune-backup-${Date.now()}.json`;
              a.click();
              URL.revokeObjectURL(url);
              toast('success', '已导出备份文件');
            } catch (e) {
              toast('error', `导出失败：${e instanceof Error ? e.message : String(e)}`);
            }
          }}
        >
          📥 导出备份
        </Button>
      </Section>
    </>
  );
}

function PrivacyPanel(): JSX.Element {
  // v0.6 Phase A.5.5：Privacy tier 状态拉取
  const privacyTier = useSignal<{
    hardware_tier?: string;
    available_layers?: string[];
    l1_regex_available?: boolean;
    l2_ner_available?: boolean;
    l3_llm_available?: boolean;
    upgrade_hint?: string | null;
  } | null>(null);
  const protectedItems = useSignal<{ count?: number; items?: string[] } | null>(null);
  const auditCount = useSignal<number>(0);

  const refreshPrivacy = async () => {
    try {
      const t = await api.get<typeof privacyTier.value>('/privacy/tier');
      privacyTier.value = t;
      const p = await api.get<{ count: number; items: string[] }>('/items/protected');
      protectedItems.value = p;
      const a = await api.get<{ total: number }>('/audit/outbound?limit=1');
      auditCount.value = a.total || 0;
    } catch (e) {
      // best-effort, silent
    }
  };

  // 首次挂载拉取
  if (privacyTier.value === null && vaultState.value === 'unlocked') {
    refreshPrivacy();
  }

  return (
    <>
      <Section title="安全">
        <SettingRow label="状态">
          <span style={{ fontSize: 'var(--text-sm)' }}>
            {vaultState.value === 'unlocked' ? '✓ 已解锁' : '🔒 已锁定'}
          </span>
        </SettingRow>
        <SettingRow label="" >
          <Button
            variant="danger"
            size="sm"
            onClick={async () => {
              if (!confirm('锁定后需要重新输入主密码解锁。')) return;
              try {
                await api.post('/vault/lock');
                clearToken();
                location.reload();
              } catch (e) {
                toast('error', `锁定失败：${e instanceof Error ? e.message : String(e)}`);
              }
            }}
          >
            🔒 锁定知识库
          </Button>
        </SettingRow>
      </Section>

      {/* v0.6 Phase A.5.5: 隐私分级状态 */}
      <Section title="数据脱敏">
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 12px 0', lineHeight: 1.5 }}>
          在发送到云端大模型前，会自动剔除敏感信息（手机号、身份证号、地址等）。
        </p>
        <SettingRow label="基础脱敏">
          <span style={{ fontSize: 'var(--text-sm)', color: privacyTier.value?.l1_regex_available ? 'var(--color-success)' : 'var(--color-text-secondary)' }}>
            {privacyTier.value?.l1_regex_available ? '✓ 已启用（12 类常见敏感信息）' : '— 未就绪'}
          </span>
        </SettingRow>
        <SettingRow label="增强脱敏">
          <span style={{ fontSize: 'var(--text-sm)' }}>
            {privacyTier.value?.l2_ner_available ? '✓ 已启用' : '🟡 后续版本启用'}
          </span>
        </SettingRow>
        <SettingRow label="智能脱敏">
          <span style={{ fontSize: 'var(--text-sm)' }}>
            {privacyTier.value?.l3_llm_available ? '✓ 已启用' : '🟡 需高端硬件或一体机'}
          </span>
        </SettingRow>
        {privacyTier.value?.upgrade_hint ? (
          <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', margin: '8px 0 0 0', fontStyle: 'italic' }}>
            💡 {privacyTier.value.upgrade_hint}
          </p>
        ) : <></>}
      </Section>

      <Section title="🔒 机密文件">
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 8px 0', lineHeight: 1.5 }}>
          标记为机密的文件不会发送到云端大模型，仅本地处理。
          目前已标记: <strong>{protectedItems.value?.count ?? 0}</strong> 个文件。
        </p>
        <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', margin: 0, lineHeight: 1.5 }}>
          标记方法：在文件列表右键文件 → "标记为机密"。
        </p>
      </Section>

      <Section title="云端调用记录">
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 12px 0', lineHeight: 1.5 }}>
          每次调用云端大模型都会在本地记录摘要（模型名 + 调用时长 + 脱敏统计），<strong>不保存原文</strong>。
          已记录: <strong>{auditCount.value}</strong> 条。
        </p>
        <SettingRow label="导出记录">
          <Button
            variant="primary"
            size="sm"
            onClick={() => {
              window.open('/api/v1/audit/outbound/export.csv', '_blank');
            }}
          >
            📥 下载 CSV
          </Button>
        </SettingRow>
      </Section>

      <Section title="遥测">
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: 0 }}>
          Attune 默认关闭所有遥测。后续版本可 opt-in 匿名使用统计。
        </p>
      </Section>
    </>
  );
}

function AboutPanel(): JSX.Element {
  const hw = hardware.value;
  return (
    <>
      <Section title="Attune">
        <p
          style={{
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text-secondary)',
            margin: 0,
            lineHeight: 1.6,
          }}
        >
          私有 AI 知识伙伴 · 本地决定，全网增强，越用越懂你的专业。
        </p>
        <SettingRow label="版本">
          <code style={codeStyle}>0.6.0-dev</code>
        </SettingRow>
        <SettingRow label="开源协议">
          <code style={codeStyle}>Apache-2.0</code>
        </SettingRow>
      </Section>
      {hw && (
        <Section title="硬件">
          <SettingRow label="CPU">
            <code style={codeStyle}>{String(hw.cpu_model ?? '—')}</code>
          </SettingRow>
          <SettingRow label="GPU">
            <code style={codeStyle}>{String(hw.gpu_model ?? '—')}</code>
          </SettingRow>
          <SettingRow label="RAM">
            <code style={codeStyle}>{String(hw.total_ram_gb ?? 0)} GB</code>
          </SettingRow>
        </Section>
      )}
    </>
  );
}

// ── 共享组件 ─────────────────────────────────────────────────
const selectStyle: JSX.CSSProperties = {
  padding: '4px var(--space-2)',
  fontSize: 'var(--text-sm)',
  background: 'var(--color-surface)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-sm)',
};

const inputStyle: JSX.CSSProperties = {
  padding: '4px var(--space-2)',
  fontSize: 'var(--text-sm)',
  background: 'var(--color-surface)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-sm)',
  color: 'var(--color-text)',
  minWidth: 280,
  fontFamily: 'var(--font-mono)',
};

const codeStyle: JSX.CSSProperties = {
  padding: '2px 6px',
  fontFamily: 'var(--font-mono)',
  fontSize: 'var(--text-xs)',
  background: 'var(--color-bg)',
  borderRadius: 'var(--radius-sm)',
  color: 'var(--color-text-secondary)',
};

function SettingRow({
  label,
  children,
}: {
  label: string;
  children?: JSX.Element | string;
}): JSX.Element {
  return (
    <div
      style={{
        display: 'flex',
        justifyContent: 'space-between',
        alignItems: 'center',
        padding: 'var(--space-3) var(--space-4)',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
      }}
    >
      {label && (
        <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text)' }}>
          {label}
        </span>
      )}
      {children}
    </div>
  );
}

function Toggle({
  value,
  onChange,
}: {
  value: boolean;
  onChange: (v: boolean) => void;
}): JSX.Element {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={value}
      onClick={() => onChange(!value)}
      style={{
        width: 40,
        height: 22,
        background: value ? 'var(--color-accent)' : 'var(--color-border)',
        borderRadius: 11,
        border: 'none',
        position: 'relative',
        cursor: 'pointer',
        transition: 'background var(--duration-fast) var(--ease-out)',
      }}
    >
      <span
        style={{
          position: 'absolute',
          top: 2,
          left: value ? 20 : 2,
          width: 18,
          height: 18,
          borderRadius: '50%',
          background: 'white',
          transition: 'left var(--duration-fast) var(--ease-out)',
          boxShadow: '0 1px 3px rgba(0,0,0,0.15)',
        }}
      />
    </button>
  );
}

// ============ Member Panel ============

function MemberPanel(): JSX.Element {
  useEffect(() => {
    void loadMemberState();
    void loadSettingsLocks();
  }, []);

  const m = memberState.value;

  const email = useSignal('');
  const password = useSignal('');
  const logging = useSignal(false);

  return (
    <div>
      <Section title="会员状态">
        {!m && <p style={{ color: 'var(--color-text-secondary)' }}>加载中...</p>}
        {m && m.is_logged_in && (
          <>
            <p>
              状态:{' '}
              <strong style={{ color: m.is_paid ? 'var(--color-accent)' : 'var(--color-text)' }}>
                {m.kind === 'paid' ? '付费会员' : '免费会员'}
              </strong>
            </p>
            {m.account_id && <p>账号: <code>{m.account_id}</code></p>}
            {m.license_id && <p>License: <code>{m.license_id}</code></p>}
            <Button
              variant="ghost"
              onClick={async () => {
                if (await memberLogout()) toast('success', '已登出');
                else toast('error', '登出失败');
              }}
            >
              登出
            </Button>
          </>
        )}
        {m && !m.is_logged_in && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxWidth: 320 }}>
            <p style={{ color: 'var(--color-text-secondary)', margin: '0 0 4px' }}>
              登录 Attune 账号以激活会员权益
            </p>
            <input
              type="email"
              placeholder="邮箱"
              value={email.value}
              onInput={(e) => { email.value = (e.target as HTMLInputElement).value; }}
              style={{
                padding: '6px 10px',
                borderRadius: 6,
                border: '1px solid var(--color-border)',
                background: 'var(--color-input-bg)',
                color: 'var(--color-text)',
                fontSize: 'var(--text-sm)',
              }}
            />
            <input
              type="password"
              placeholder="密码"
              value={password.value}
              onInput={(e) => { password.value = (e.target as HTMLInputElement).value; }}
              onKeyDown={async (e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  await doLogin();
                }
              }}
              style={{
                padding: '6px 10px',
                borderRadius: 6,
                border: '1px solid var(--color-border)',
                background: 'var(--color-input-bg)',
                color: 'var(--color-text)',
                fontSize: 'var(--text-sm)',
              }}
            />
            <Button
              variant="primary"
              disabled={logging.value || !email.value || !password.value}
              onClick={doLogin}
            >
              {logging.value ? '登录中...' : '登录'}
            </Button>
          </div>
        )}
      </Section>

      {/* 锁定状态体现在各 tab 的实际字段位置 (灰色 + 🔒), 不再单独矩阵显示 */}
    </div>
  );

  async function doLogin() {
    if (!email.value || !password.value) return;
    logging.value = true;
    const result = await memberLoginPassword(email.value.trim(), password.value);
    logging.value = false;
    if (result.ok) {
      toast('success', '登录成功');
      email.value = '';
      password.value = '';
    } else {
      toast('error', `登录失败：${result.error ?? '未知错误'}`);
    }
  }
}

// ============ Folder Links Panel (注入 DataPanel 用, 这里也单独 export) ============

export function FolderLinksSection(): JSX.Element {
  const picking = useSignal(false);
  const canPickFolder = typeof window !== 'undefined'
    && Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);

  useEffect(() => {
    void loadFolderLinks();
  }, []);

  async function onAddFolder(): Promise<void> {
    if (!canPickFolder) {
      toast('warning', '请在桌面应用窗口中添加本地目录');
      return;
    }
    picking.value = true;
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({ directory: true, multiple: true, title: '选择要关联的文件夹' });
      const chosen = Array.isArray(selected) ? selected : selected ? [selected] : [];
      let added = 0;
      for (const path of chosen) {
        try {
          await api.post('/index/bind', { path, recursive: true });
          added += 1;
        } catch {
          // 单个失败不阻塞其它
        }
      }
      if (added > 0) {
        toast('success', `已添加 ${added} 个目录，开始自动入库`);
        await loadFolderLinks();
      }
    } catch (e) {
      toast('error', e instanceof Error ? e.message : '添加失败');
    } finally {
      picking.value = false;
    }
  }

  const links = folderLinks.value;
  return (
    <Section
      title="本地知识库目录"
      desc="已关联的本地文件夹会被自动监听、自动入库"
    >
      <div style={{ marginBottom: 'var(--space-3)' }}>
        <Button
          variant="primary"
          size="sm"
          disabled={picking.value || !canPickFolder}
          onClick={() => void onAddFolder()}
        >
          {picking.value ? '正在打开文件夹选择…' : '+ 添加本地目录'}
        </Button>
        {!canPickFolder && (
          <span style={{ marginLeft: 'var(--space-3)', fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
            （仅桌面应用窗口内可用；浏览器调试模式无文件夹弹窗）
          </span>
        )}
      </div>
      {links.length === 0 && (
        <p style={{ color: 'var(--color-text-secondary)' }}>
          尚未关联任何本地目录。
        </p>
      )}
      {links.length > 0 && (
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 'var(--text-sm)' }}>
          <thead>
            <tr style={{ textAlign: 'left', borderBottom: '1px solid var(--color-border)' }}>
              <th style={{ padding: 8 }}>路径</th>
              <th style={{ padding: 8 }}>项目</th>
              <th style={{ padding: 8 }}>添加时间</th>
            </tr>
          </thead>
          <tbody>
            {links.map((fl, idx) => (
              <tr
                key={fl.id ?? `${fl.path}-${idx}`}
                style={{ borderBottom: '1px solid var(--color-border-subtle)' }}
              >
                <td style={{ padding: 8, fontFamily: 'monospace' }}>{fl.path}</td>
                <td style={{ padding: 8 }}>{fl.project_id ?? '默认'}</td>
                <td style={{ padding: 8 }}>{fl.added_at ?? '—'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Section>
  );
}
