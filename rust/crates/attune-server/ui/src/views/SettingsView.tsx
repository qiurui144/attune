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
  currentView,
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
  labelKey: string;
  endpoint: string;
  model: string;
}

const LLM_PRESETS: Record<LlmPresetKey, LlmPreset> = {
  custom: { labelKey: 'settings.ai.llm.preset.custom', endpoint: '', model: '' },
  deepseek: {
    labelKey: 'settings.ai.llm.preset.deepseek',
    endpoint: 'https://api.deepseek.com/v1',
    model: 'deepseek-chat',
  },
  qwen: {
    labelKey: 'settings.ai.llm.preset.qwen',
    endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    model: 'qwen-plus',
  },
  glm: {
    labelKey: 'settings.ai.llm.preset.glm',
    endpoint: 'https://open.bigmodel.cn/api/paas/v4',
    model: 'glm-4-plus',
  },
  kimi: {
    labelKey: 'settings.ai.llm.preset.kimi',
    endpoint: 'https://api.moonshot.cn/v1',
    model: 'moonshot-v1-8k',
  },
  baichuan: {
    labelKey: 'settings.ai.llm.preset.baichuan',
    endpoint: 'https://api.baichuan-ai.com/v1',
    model: 'Baichuan4-Turbo',
  },
  ollama: {
    labelKey: 'settings.ai.llm.preset.ollama',
    endpoint: 'http://localhost:11434/v1',
    model: 'auto',
  },
  openai: {
    labelKey: 'settings.ai.llm.preset.openai',
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
      // 硬件字段嵌在 diagnostics 响应的 hardware 子对象下
      .then((d) => (hardware.value = (d.hardware as Record<string, unknown> | undefined) ?? d))
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
            {t('settings.ai.llm.locked_notice')}
          </div>
        )}
        <SettingRow label={t('settings.ai.llm.preset')}>
          <select
            value={presetKey.value}
            disabled={llmLocked}
            onChange={(e) => onPresetChange(e.currentTarget.value as LlmPresetKey)}
            style={{ ...selectStyle, minWidth: 240, ...(llmLocked ? { background: 'var(--color-surface-muted, #f4f5f7)', cursor: 'not-allowed' } : {}) }}
            aria-label={t('settings.ai.llm.preset_aria')}
          >
            {(Object.keys(LLM_PRESETS) as LlmPresetKey[]).map((k) => (
              <option key={k} value={k}>
                {t(LLM_PRESETS[k].labelKey)}
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
            placeholder={t('settings.ai.llm.endpoint_placeholder')}
            style={lockedInputStyle}
          />
        </SettingRow>
        <SettingRow label={t('settings.ai.llm.model')}>
          <input
            type="text"
            value={draftModel.value}
            disabled={llmLocked}
            onInput={(e) => (draftModel.value = e.currentTarget.value)}
            placeholder={t('settings.ai.llm.model_placeholder')}
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

      <Section title={t('settings.ai.query_rewrite.title')}>
        <SettingRow label={t('settings.ai.query_rewrite.enabled')}>
          <Toggle
            value={
              Boolean(
                ((settings.value?.search as { query_rewrite?: { enabled?: boolean } })
                  ?.query_rewrite)?.enabled,
              )
            }
            onChange={async (v) => {
              await patchSettings({
                search: {
                  ...(settings.value?.search as Record<string, unknown>),
                  query_rewrite: { enabled: v },
                },
              });
              toast('success', v ? t('settings.ai.query_rewrite.enabled_toast') : t('settings.ai.query_rewrite.disabled_toast'));
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
      <Section title={t('settings.data.source.title')}>
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: 0 }}>
          {t('settings.data.source.desc')}
        </p>
      </Section>
      <FolderLinksSection />
      <Section title={t('settings.data.backup.title')}>
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 8px 0' }}>
          {t('settings.data.backup.desc')}
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
              toast('success', t('settings.data.backup.export_ok'));
            } catch (e) {
              toast('error', t('settings.data.backup.export_fail', { message: e instanceof Error ? e.message : String(e) }));
            }
          }}
        >
          {t('settings.data.backup.export_btn')}
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
      {/* v1.0.6 — link to dedicated Privacy dashboard (5 outbound points + DSAR) */}
      <Section title={t('privacy.title')}>
        <SettingRow label={t('privacy.subtitle')}>
          <Button
            variant="primary"
            size="sm"
            onClick={() => {
              currentView.value = 'privacy';
            }}
          >
            {t('privacy.outbound.title')}
          </Button>
        </SettingRow>
      </Section>
      <Section title={t('settings.privacy.security.title')}>
        <SettingRow label={t('settings.privacy.security.status')}>
          <span style={{ fontSize: 'var(--text-sm)' }}>
            {vaultState.value === 'unlocked' ? t('settings.privacy.security.unlocked') : t('settings.privacy.security.locked')}
          </span>
        </SettingRow>
        <SettingRow label="" >
          <Button
            variant="danger"
            size="sm"
            onClick={async () => {
              if (!confirm(t('settings.privacy.security.lock_confirm'))) return;
              try {
                await api.post('/vault/lock');
                clearToken();
                location.reload();
              } catch (e) {
                toast('error', t('settings.privacy.security.lock_fail', { message: e instanceof Error ? e.message : String(e) }));
              }
            }}
          >
            {t('settings.privacy.security.lock_btn')}
          </Button>
        </SettingRow>
      </Section>

      {/* v0.6 Phase A.5.5: 隐私分级状态 */}
      <Section title={t('settings.privacy.redact.title')}>
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 12px 0', lineHeight: 1.5 }}>
          {t('settings.privacy.redact.desc')}
        </p>
        <SettingRow label={t('settings.privacy.redact.basic')}>
          <span style={{ fontSize: 'var(--text-sm)', color: privacyTier.value?.l1_regex_available ? 'var(--color-success)' : 'var(--color-text-secondary)' }}>
            {privacyTier.value?.l1_regex_available ? t('settings.privacy.redact.basic_on') : t('settings.privacy.redact.basic_off')}
          </span>
        </SettingRow>
        <SettingRow label={t('settings.privacy.redact.enhanced')}>
          <span style={{ fontSize: 'var(--text-sm)' }}>
            {privacyTier.value?.l2_ner_available ? t('settings.privacy.redact.enhanced_on') : t('settings.privacy.redact.enhanced_off')}
          </span>
        </SettingRow>
        <SettingRow label={t('settings.privacy.redact.smart')}>
          <span style={{ fontSize: 'var(--text-sm)' }}>
            {privacyTier.value?.l3_llm_available ? t('settings.privacy.redact.smart_on') : t('settings.privacy.redact.smart_off')}
          </span>
        </SettingRow>
        {privacyTier.value?.upgrade_hint ? (
          <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', margin: '8px 0 0 0', fontStyle: 'italic' }}>
            💡 {privacyTier.value.upgrade_hint}
          </p>
        ) : <></>}
      </Section>

      <Section title={t('settings.privacy.confidential.title')}>
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 8px 0', lineHeight: 1.5 }}>
          {t('settings.privacy.confidential.desc', { count: protectedItems.value?.count ?? 0 })}
        </p>
        <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', margin: 0, lineHeight: 1.5 }}>
          {t('settings.privacy.confidential.howto')}
        </p>
      </Section>

      <Section title={t('settings.privacy.audit.title')}>
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 12px 0', lineHeight: 1.5 }}>
          {t('settings.privacy.audit.desc', { count: auditCount.value })}
        </p>
        <SettingRow label={t('settings.privacy.audit.export_label')}>
          <Button
            variant="primary"
            size="sm"
            onClick={() => {
              window.open('/api/v1/audit/outbound/export.csv', '_blank');
            }}
          >
            {t('settings.privacy.audit.export_btn')}
          </Button>
        </SettingRow>
      </Section>

      <Section title={t('settings.privacy.telemetry.title')}>
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: 0 }}>
          {t('settings.privacy.telemetry.desc')}
        </p>
      </Section>
    </>
  );
}

function AboutPanel(): JSX.Element {
  const hw = hardware.value;
  const m = memberState.value;
  // 调 ai_stack 看底座 + cloud 状态 (一次性, AboutPanel 挂载时)
  const aiStack = useSignal<Record<string, unknown> | null>(null);
  useEffect(() => {
    void api.get<Record<string, unknown>>('/ai_stack').then((d) => (aiStack.value = d)).catch(() => {});
    void loadMemberState();
  }, []);

  const stackStatus = (key: string): { ok: boolean; note?: string } => {
    const obj = aiStack.value?.[key] as { available?: boolean; note?: string } | undefined;
    return { ok: !!obj?.available, note: obj?.note };
  };
  const emb = stackStatus('embedding');
  const rer = stackStatus('rerank');
  const ocr = stackStatus('ocr');
  const asr = stackStatus('asr');
  const ws = stackStatus('web_search');

  // 数据目录: Linux/macOS = ~/.local/share/attune, Windows = %APPDATA%\attune
  const dataDir = (() => {
    if (typeof navigator !== 'undefined' && navigator.platform.toLowerCase().includes('win')) {
      return '%APPDATA%\\attune';
    }
    return '~/.local/share/attune';
  })();

  const memberLabel = !m
    ? t('settings.about.member.not_logged_in')
    : m.kind === 'paid'
      ? t('settings.about.member.paid', { account: m.account_id ?? '-' })
      : m.kind === 'free'
        ? t('settings.about.member.free', { account: m.account_id ?? '-' })
        : t('settings.about.member.not_logged_in');

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
          {t('settings.about.app.tagline')}
        </p>
        <SettingRow label={t('settings.about.app.version')}>
          <code style={codeStyle}>0.7.0-dev</code>
        </SettingRow>
        <SettingRow label={t('settings.about.app.license')}>
          <code style={codeStyle}>Apache-2.0</code>
        </SettingRow>
        <SettingRow label={t('settings.about.app.member')}>
          <span style={{ fontSize: 'var(--text-sm)' }}>{memberLabel}</span>
        </SettingRow>
      </Section>

      <Section title={t('settings.about.services.title')}>
        <SettingRow label={t('settings.about.services.vector')}>
          <ServiceStatus ok={emb.ok} note={emb.note} />
        </SettingRow>
        <SettingRow label={t('settings.about.services.rerank')}>
          <ServiceStatus ok={rer.ok} note={rer.note} />
        </SettingRow>
        <SettingRow label={t('settings.about.services.ocr')}>
          <ServiceStatus ok={ocr.ok} note={ocr.note} />
        </SettingRow>
        <SettingRow label={t('settings.about.services.asr')}>
          <ServiceStatus ok={asr.ok} note={asr.note} />
        </SettingRow>
        <SettingRow label={t('settings.about.services.web_search')}>
          <ServiceStatus ok={ws.ok} note={ws.note} />
        </SettingRow>
      </Section>

      <Section title={t('settings.about.storage.title')}>
        <SettingRow label={t('settings.about.storage.data_dir')}>
          <code style={codeStyle}>{dataDir}</code>
        </SettingRow>
        <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', margin: 0, lineHeight: 1.5 }}>
          {t('settings.about.storage.note')}
        </p>
      </Section>

      {hw && (
        <Section title={t('settings.about.hardware.title')}>
          <SettingRow label={t('settings.about.hardware.cpu')}>
            <code style={codeStyle}>{String(hw.cpu_model ?? '—')}</code>
          </SettingRow>
          <SettingRow label={t('settings.about.hardware.gpu')}>
            <code style={codeStyle}>{String(hw.gpu_label ?? '—')}</code>
          </SettingRow>
          <SettingRow label={t('settings.about.hardware.ram')}>
            <code style={codeStyle}>{String(hw.total_ram_gb ?? 0)} GB</code>
          </SettingRow>
        </Section>
      )}

      <Section title={t('settings.about.help.title')}>
        <SettingRow label={t('settings.about.help.docs')}>
          <a href="https://github.com/qiurui144/attune#readme" target="_blank" rel="noopener noreferrer"
             style={{ color: 'var(--color-accent)', fontSize: 'var(--text-sm)' }}>
            {t('settings.about.help.docs_link')}
          </a>
        </SettingRow>
        <SettingRow label={t('settings.about.help.feedback')}>
          <a href="https://github.com/qiurui144/attune/issues" target="_blank" rel="noopener noreferrer"
             style={{ color: 'var(--color-accent)', fontSize: 'var(--text-sm)' }}>
            {t('settings.about.help.feedback_link')}
          </a>
        </SettingRow>
        <SettingRow label={t('settings.about.help.source')}>
          <a href="https://github.com/qiurui144/attune" target="_blank" rel="noopener noreferrer"
             style={{ color: 'var(--color-accent)', fontSize: 'var(--text-sm)' }}>
            github.com/qiurui144/attune
          </a>
        </SettingRow>
      </Section>
    </>
  );
}

function ServiceStatus({ ok, note }: { ok: boolean; note?: string }): JSX.Element {
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 'var(--space-2)', fontSize: 'var(--text-sm)' }}>
      {ok ? (
        <span style={{ color: 'var(--color-success)' }}>{t('settings.about.services.ready')}</span>
      ) : (
        <span style={{ color: 'var(--color-warning)' }}>{t('settings.about.services.not_ready')}</span>
      )}
      {note && !ok && (
        <span style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>{note}</span>
      )}
    </span>
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

  // FEAT-1: 自部署 cloud endpoint 配置 (UX gap 关闭).
  // 默认 engi-stack.com, 自部署用户填入私有 cluster URL.
  const showAdvancedCloud = useSignal(false);
  const cloudAccountsUrl = useSignal('');
  const cloudGatewayUrl = useSignal('');
  const cloudPluginhubUrl = useSignal('');
  // 自部署 pluginhub 需 url + license_key 两者齐全才切到 HttpPluginHubProvider
  const cloudPluginhubKey = useSignal('');

  useEffect(() => {
    // 初始化时把 settings 已有的 cloud config 显示到 input
    const s = settings.value as Record<string, unknown> | null;
    if (s && typeof s === 'object') {
      const cloud = s['cloud'] as Record<string, string | null> | undefined;
      const pluginhub = s['pluginhub'] as Record<string, string | null> | undefined;
      if (cloud?.accounts_url) cloudAccountsUrl.value = cloud.accounts_url ?? '';
      if (cloud?.gateway_url) cloudGatewayUrl.value = cloud.gateway_url ?? '';
      if (pluginhub?.url) cloudPluginhubUrl.value = pluginhub.url ?? '';
      if (pluginhub?.license_key) cloudPluginhubKey.value = pluginhub.license_key ?? '';
    }
  }, [settings.value]);

  async function saveCloudConfig() {
    const patch: Record<string, unknown> = {
      cloud: {
        accounts_url: cloudAccountsUrl.value.trim() || null,
        gateway_url: cloudGatewayUrl.value.trim() || null,
      },
    };
    if (cloudPluginhubUrl.value.trim()) {
      patch.pluginhub = {
        url: cloudPluginhubUrl.value.trim(),
        license_key: cloudPluginhubKey.value.trim() || null,
      };
    }
    const ok = await patchSettings(patch);
    if (ok) toast('success', t('settings.member.cloud.save_ok'));
    else toast('error', t('settings.member.cloud.save_fail'));
  }

  return (
    <div>
      <Section title={t('settings.member.status.title')}>
        {!m && <p style={{ color: 'var(--color-text-secondary)' }}>{t('settings.member.status.loading')}</p>}
        {m && m.is_logged_in && (
          <>
            <p>
              {t('settings.member.status.label')}{' '}
              <strong style={{ color: m.is_paid ? 'var(--color-accent)' : 'var(--color-text)' }}>
                {m.kind === 'paid' ? t('settings.member.status.paid') : t('settings.member.status.free')}
              </strong>
            </p>
            {m.account_id && <p>{t('settings.member.account')} <code>{m.account_id}</code></p>}
            {m.license_id && <p>{t('settings.member.license')} <code>{m.license_id}</code></p>}
            <Button
              variant="ghost"
              onClick={async () => {
                if (await memberLogout()) toast('success', t('settings.member.logout_ok'));
                else toast('error', t('settings.member.logout_fail'));
              }}
            >
              {t('settings.member.logout')}
            </Button>
          </>
        )}
        {m && !m.is_logged_in && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxWidth: 320 }}>
            <p style={{ color: 'var(--color-text-secondary)', margin: '0 0 4px' }}>
              {t('settings.member.login_prompt')}
            </p>
            <input
              type="email"
              placeholder={t('settings.member.email_placeholder')}
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
              placeholder={t('settings.member.password_placeholder')}
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
              {logging.value ? t('settings.member.logging_in') : t('settings.member.login')}
            </Button>
          </div>
        )}
      </Section>

      {/* 锁定状态体现在各 tab 的实际字段位置 (灰色 + 🔒), 不再单独矩阵显示 */}

      {/* FEAT-1: 自部署 cloud 后端地址 (折叠, 默认隐藏 — 仅自部署 / 企业内网用户需要) */}
      <Section title={t('settings.member.cloud.title')}>
        <button
          onClick={() => { showAdvancedCloud.value = !showAdvancedCloud.value; }}
          style={{
            background: 'none',
            border: 'none',
            color: 'var(--color-accent)',
            cursor: 'pointer',
            padding: 0,
            fontSize: 'var(--text-sm)',
            marginBottom: 8,
          }}
        >
          {showAdvancedCloud.value ? t('settings.member.cloud.hide') : t('settings.member.cloud.show')}{t('settings.member.cloud.default_note')}
        </button>
        {showAdvancedCloud.value && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxWidth: 480 }}>
            <p style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-sm)', margin: '0 0 4px' }}>
              {t('settings.member.cloud.desc')}
            </p>
            <label style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>
              {t('settings.member.cloud.accounts_label')}
            </label>
            <input
              type="url"
              placeholder="https://accounts.your-company.com"
              value={cloudAccountsUrl.value}
              onInput={(e) => { cloudAccountsUrl.value = (e.target as HTMLInputElement).value; }}
              style={{
                padding: '6px 10px',
                borderRadius: 6,
                border: '1px solid var(--color-border)',
                background: 'var(--color-input-bg)',
                color: 'var(--color-text)',
                fontSize: 'var(--text-sm)',
              }}
            />
            <label style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>
              {t('settings.member.cloud.gateway_label')}
            </label>
            <input
              type="url"
              placeholder="https://gateway.your-company.com"
              value={cloudGatewayUrl.value}
              onInput={(e) => { cloudGatewayUrl.value = (e.target as HTMLInputElement).value; }}
              style={{
                padding: '6px 10px',
                borderRadius: 6,
                border: '1px solid var(--color-border)',
                background: 'var(--color-input-bg)',
                color: 'var(--color-text)',
                fontSize: 'var(--text-sm)',
              }}
            />
            <label style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>
              {t('settings.member.cloud.pluginhub_label')}
            </label>
            <input
              type="url"
              placeholder="https://hub.your-company.com"
              value={cloudPluginhubUrl.value}
              onInput={(e) => { cloudPluginhubUrl.value = (e.target as HTMLInputElement).value; }}
              style={{
                padding: '6px 10px',
                borderRadius: 6,
                border: '1px solid var(--color-border)',
                background: 'var(--color-input-bg)',
                color: 'var(--color-text)',
                fontSize: 'var(--text-sm)',
              }}
            />
            <label style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)' }}>
              {t('settings.member.cloud.pluginhub_key_label')}
            </label>
            <input
              type="text"
              placeholder="license key"
              value={cloudPluginhubKey.value}
              onInput={(e) => { cloudPluginhubKey.value = (e.target as HTMLInputElement).value; }}
              style={{
                padding: '6px 10px',
                borderRadius: 6,
                border: '1px solid var(--color-border)',
                background: 'var(--color-input-bg)',
                color: 'var(--color-text)',
                fontSize: 'var(--text-sm)',
              }}
            />
            <Button variant="primary" onClick={saveCloudConfig}>
              {t('settings.member.cloud.save_btn')}
            </Button>
          </div>
        )}
      </Section>
    </div>
  );

  async function doLogin() {
    if (!email.value || !password.value) return;
    logging.value = true;
    const result = await memberLoginPassword(email.value.trim(), password.value);
    logging.value = false;
    if (result.ok) {
      toast('success', t('settings.member.login_ok'));
      email.value = '';
      password.value = '';
    } else {
      toast('error', t('settings.member.login_fail', { message: result.error ?? t('settings.member.unknown_error') }));
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
      toast('warning', t('settings.folder.desktop_only'));
      return;
    }
    picking.value = true;
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({ directory: true, multiple: true, title: t('settings.folder.pick_title') });
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
        toast('success', t('settings.folder.added', { count: added }));
        await loadFolderLinks();
      }
    } catch (e) {
      toast('error', e instanceof Error ? e.message : t('settings.folder.add_fail'));
    } finally {
      picking.value = false;
    }
  }

  const links = folderLinks.value;
  return (
    <Section
      title={t('settings.folder.title')}
      desc={t('settings.folder.desc')}
    >
      <div style={{ marginBottom: 'var(--space-3)' }}>
        <Button
          variant="primary"
          size="sm"
          disabled={picking.value || !canPickFolder}
          onClick={() => void onAddFolder()}
        >
          {picking.value ? t('settings.folder.opening') : t('settings.folder.add_btn')}
        </Button>
        {!canPickFolder && (
          <span style={{ marginLeft: 'var(--space-3)', fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
            {t('settings.folder.browser_note')}
          </span>
        )}
      </div>
      {links.length === 0 && (
        <p style={{ color: 'var(--color-text-secondary)' }}>
          {t('settings.folder.empty')}
        </p>
      )}
      {links.length > 0 && (
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 'var(--text-sm)' }}>
          <thead>
            <tr style={{ textAlign: 'left', borderBottom: '1px solid var(--color-border)' }}>
              <th style={{ padding: 8 }}>{t('settings.folder.col_path')}</th>
              <th style={{ padding: 8 }}>{t('settings.folder.col_project')}</th>
              <th style={{ padding: 8 }}>{t('settings.folder.col_added')}</th>
            </tr>
          </thead>
          <tbody>
            {links.map((fl, idx) => (
              <tr
                key={fl.id ?? `${fl.path}-${idx}`}
                style={{ borderBottom: '1px solid var(--color-border-subtle)' }}
              >
                <td style={{ padding: 8, fontFamily: 'monospace' }}>{fl.path}</td>
                <td style={{ padding: 8 }}>{fl.project_id ?? t('settings.folder.default_project')}</td>
                <td style={{ padding: 8 }}>{fl.added_at ?? '—'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Section>
  );
}
