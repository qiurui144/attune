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
  ocrProfiles,
  folderLinks,
  type OcrProfile,
} from '../store/signals';
import { setLocale, currentLocale, t } from '../i18n';
import { loadSettings, patchSettings } from '../hooks/useSettings';
import { loadMemberState, loadSettingsLocks, memberLogout } from '../hooks/useMember';
import {
  loadOcrProfiles,
  createOcrProfile,
  updateOcrProfile,
  deleteOcrProfile,
} from '../hooks/useOcrProfiles';
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
    model: 'qwen2.5:7b',
  },
  openai: {
    label: 'OpenAI (~¥3/M tok)',
    endpoint: 'https://api.openai.com/v1',
    model: 'gpt-4o-mini',
  },
};

type SettingsTab = 'general' | 'ai' | 'data' | 'ocr' | 'member' | 'privacy' | 'about';

const TABS: Array<{ key: SettingsTab; icon: string; label: string }> = [
  { key: 'general', icon: '⚙', label: '通用' },
  { key: 'ai', icon: '🤖', label: 'AI 大脑' },
  { key: 'ocr', icon: '📷', label: 'OCR 场景' },
  { key: 'data', icon: '📂', label: '数据' },
  { key: 'member', icon: '👤', label: '会员' },
  { key: 'privacy', icon: '🔐', label: '隐私' },
  { key: 'about', icon: 'ℹ', label: '关于' },
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
    <div style={{ height: '100%', display: 'flex' }}>
      <nav
        aria-label="Settings sections"
        style={{
          width: 200,
          flexShrink: 0,
          borderRight: '1px solid var(--color-border)',
          padding: 'var(--space-5) 0',
          background: 'var(--color-bg)',
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
          设置
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
              <span>{tab.label}</span>
            </button>
          );
        })}
      </nav>

      <div
        style={{
          flex: 1,
          overflow: 'auto',
          padding: 'var(--space-6) var(--space-7)',
        }}
      >
        {activeTab.value === 'general' && <GeneralPanel />}
        {activeTab.value === 'ai' && <AIPanel />}
        {activeTab.value === 'ocr' && <OcrPanel />}
        {activeTab.value === 'data' && <DataPanel />}
        {activeTab.value === 'member' && <MemberPanel />}
        {activeTab.value === 'privacy' && <PrivacyPanel />}
        {activeTab.value === 'about' && <AboutPanel />}
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
  const emb = useComputed(() => (settings.value?.embedding as Record<string, unknown>) ?? {});

  // 编辑态（草稿值，保存按钮才下发）
  const presetKey = useSignal<LlmPresetKey>('custom');
  const draftEndpoint = useSignal<string>('');
  const draftModel = useSignal<string>('');
  const draftApiKey = useSignal<string>('');
  const saving = useSignal(false);

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
        toast('success', '已保存 LLM 配置');
      } else {
        toast('error', '保存失败');
      }
    } finally {
      saving.value = false;
    }
  };

  return (
    <>
      <Section title="LLM 后端">
        <SettingRow label="快捷预设">
          <select
            value={presetKey.value}
            onChange={(e) => onPresetChange(e.currentTarget.value as LlmPresetKey)}
            style={{ ...selectStyle, minWidth: 240 }}
            aria-label="LLM 厂商快捷预设"
          >
            {(Object.keys(LLM_PRESETS) as LlmPresetKey[]).map((k) => (
              <option key={k} value={k}>
                {LLM_PRESETS[k].label}
              </option>
            ))}
          </select>
        </SettingRow>
        <SettingRow label="Endpoint">
          <input
            type="text"
            value={draftEndpoint.value}
            onInput={(e) => (draftEndpoint.value = e.currentTarget.value)}
            placeholder="https://api.example.com/v1"
            style={inputStyle}
          />
        </SettingRow>
        <SettingRow label="Chat 模型">
          <input
            type="text"
            value={draftModel.value}
            onInput={(e) => (draftModel.value = e.currentTarget.value)}
            placeholder="例：deepseek-chat / qwen-plus / gpt-4o-mini"
            style={inputStyle}
          />
        </SettingRow>
        <SettingRow label="API Key">
          <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'center' }}>
            <input
              type="password"
              value={draftApiKey.value}
              onInput={(e) => (draftApiKey.value = e.currentTarget.value)}
              placeholder={llm.value.api_key_set ? '已配置（留空保留）' : '粘贴 sk-... '}
              style={inputStyle}
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
            disabled={saving.value}
          >
            {saving.value ? '保存中…' : '💾 保存 LLM 配置'}
          </Button>
        </SettingRow>
      </Section>

      <Section title="Embedding">
        <SettingRow label="模型">
          <code style={codeStyle}>{(emb.value.model as string) ?? '—'}</code>
        </SettingRow>
        <SettingRow label="Ollama URL">
          <code style={codeStyle}>{(emb.value.ollama_url as string) ?? '—'}</code>
        </SettingRow>
      </Section>

      <Section title="网络搜索">
        <SettingRow label="启用">
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
              toast('success', v ? '已启用网络搜索' : '已关闭网络搜索');
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
      <Section title="导入 / 导出">
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
              a.download = `attune-profile-${Date.now()}.vault-profile`;
              a.click();
              URL.revokeObjectURL(url);
              toast('success', '已导出 profile');
            } catch (e) {
              toast('error', `导出失败：${e instanceof Error ? e.message : String(e)}`);
            }
          }}
        >
          📥 导出 .vault-profile
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
        <SettingRow label="Vault 状态">
          <span style={{ fontSize: 'var(--text-sm)' }}>
            {vaultState.value === 'unlocked' ? '✓ 已解锁' : '🔒 已锁定'}
          </span>
        </SettingRow>
        <SettingRow label="" >
          <Button
            variant="danger"
            size="sm"
            onClick={async () => {
              if (!confirm('锁定后需要重新输入 Master Password 解锁。')) return;
              try {
                await api.post('/vault/lock');
                clearToken();
                location.reload();
              } catch (e) {
                toast('error', `锁定失败：${e instanceof Error ? e.message : String(e)}`);
              }
            }}
          >
            🔒 锁定 vault
          </Button>
        </SettingRow>
      </Section>

      {/* v0.6 Phase A.5.5: 隐私分级状态 */}
      <Section title="隐私分级 (Phase A.5)">
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 12px 0', lineHeight: 1.5 }}>
          Attune 的"成本/隐私三层模型"：
        </p>
        <SettingRow label="L1 正则脱敏">
          <span style={{ fontSize: 'var(--text-sm)', color: privacyTier.value?.l1_regex_available ? 'var(--color-success)' : 'var(--color-text-secondary)' }}>
            {privacyTier.value?.l1_regex_available ? '✓ 默认启用 (12 类格式化 PII)' : '— 未就绪'}
          </span>
        </SettingRow>
        <SettingRow label="L2 NER">
          <span style={{ fontSize: 'var(--text-sm)' }}>
            {privacyTier.value?.l2_ner_available ? '✓ 可用' : '🟡 v0.6 排期 (~300MB ONNX 模型)'}
          </span>
        </SettingRow>
        <SettingRow label="L3 LLM 脱敏">
          <span style={{ fontSize: 'var(--text-sm)' }}>
            {privacyTier.value?.l3_llm_available ? '✓ 可用 (Tier T3+/K3)' : '🟡 v0.7 排期，需高端硬件'}
          </span>
        </SettingRow>
        {privacyTier.value?.upgrade_hint ? (
          <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', margin: '8px 0 0 0', fontStyle: 'italic' }}>
            💡 {privacyTier.value.upgrade_hint}
          </p>
        ) : <></>}
      </Section>

      <Section title="🔒 受保护文件 (per-file L0)">
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 8px 0', lineHeight: 1.5 }}>
          标记为 L0 的文件 chunk 永不出现在云端 LLM context 里（强制本地 LLM）。
          目前已标记: <strong>{protectedItems.value?.count ?? 0}</strong> 个文件。
        </p>
        <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', margin: 0, lineHeight: 1.5 }}>
          标记方法：在文件列表右键文件 → "标记为机密" 或 PATCH /api/v1/items/{'{id}'}/privacy_tier。
        </p>
      </Section>

      <Section title="出网审计日志">
        <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', margin: '0 0 12px 0', lineHeight: 1.5 }}>
          每次云端 LLM 调用都本地落 audit log（SHA256 hash + 模型 + token 数 + 脱敏统计，<strong>0 用户原文落库</strong>）。
          已记录: <strong>{auditCount.value}</strong> 条。
        </p>
        <SettingRow label="导出 CSV">
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
        <SettingRow label="许可">
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

// ============ OCR Panel ============

function OcrPanel(): JSX.Element {
  const editing = useSignal<OcrProfile | null>(null);
  const saving = useSignal(false);

  useEffect(() => {
    void loadOcrProfiles();
    void loadSettingsLocks();
    void loadSettings();
  }, []);

  const locks = settingsLocks.value;
  const canWrite = locks ? locks.ocr_profiles === 'editable' : true;
  const activeProfile =
    ((settings.value?.ocr as Record<string, unknown> | undefined)?.active_profile as string) ??
    'contract';

  const onSetActive = async (id: string) => {
    const ok = await patchSettings({ ocr: { active_profile: id } });
    if (ok) toast('success', `默认 OCR 场景已切换为 ${id}`);
    else toast('error', '切换失败 (会员锁?)');
  };

  const onSave = async () => {
    const p = editing.value;
    if (!p) return;
    saving.value = true;
    const isNew = !ocrProfiles.value.some((x) => x.id === p.id);
    const ok = isNew ? !!(await createOcrProfile(p)) : await updateOcrProfile(p);
    saving.value = false;
    if (ok) {
      editing.value = null;
      toast('success', isNew ? '已创建' : '已更新');
    } else {
      toast('error', '保存失败');
    }
  };

  const onDelete = async (id: string) => {
    if (!confirm(`确认删除 OCR profile "${id}"？`)) return;
    const ok = await deleteOcrProfile(id);
    if (ok) toast('success', '已删除');
    else toast('error', '删除失败 (builtin 不可删?)');
  };

  return (
    <div>
      <Section
        title="OCR 场景预设"
        desc="按文档类型选不同 DPI / 标签 — 票据(200) / 合同(300) / 古籍(600). builtin 预设不可改不可删."
      >
        {!canWrite && (
          <p style={{ fontSize: 'var(--text-sm)', color: 'var(--color-warning)' }}>
            ⚠ 当前会员等级已锁定 OCR profile 修改 (GET /api/v1/member/locks)
          </p>
        )}
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 'var(--text-sm)' }}>
          <thead>
            <tr style={{ textAlign: 'left', borderBottom: '1px solid var(--color-border)' }}>
              <th style={{ padding: 8 }}>ID</th>
              <th style={{ padding: 8 }}>名称</th>
              <th style={{ padding: 8 }}>DPI</th>
              <th style={{ padding: 8 }}>类型</th>
              <th style={{ padding: 8 }}>操作</th>
            </tr>
          </thead>
          <tbody>
            {ocrProfiles.value.map((p) => (
              <tr key={p.id} style={{ borderBottom: '1px solid var(--color-border-subtle)' }}>
                <td style={{ padding: 8, fontFamily: 'monospace' }}>
                  {p.id}
                  {activeProfile === p.id && (
                    <span style={{ color: 'var(--color-accent)', marginLeft: 6 }}>● 默认</span>
                  )}
                </td>
                <td style={{ padding: 8 }}>{p.name}</td>
                <td style={{ padding: 8 }}>{p.dpi}</td>
                <td style={{ padding: 8 }}>{p.builtin ? '🔒 builtin' : '✏️ custom'}</td>
                <td style={{ padding: 8 }}>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={!canWrite || activeProfile === p.id}
                    onClick={() => void onSetActive(p.id)}
                  >
                    设默认
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={p.builtin || !canWrite}
                    onClick={() => (editing.value = { ...p })}
                  >
                    编辑
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={p.builtin || !canWrite}
                    onClick={() => void onDelete(p.id)}
                  >
                    删除
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <div style={{ marginTop: 12 }}>
          <Button
            variant="primary"
            disabled={!canWrite}
            onClick={() =>
              (editing.value = {
                id: '',
                name: '',
                description: '',
                languages: 'chi_sim+eng',
                dpi: 300,
                tags: [],
                builtin: false,
              })
            }
          >
            + 新建场景
          </Button>
        </div>
      </Section>

      {editing.value && (
        <Section title={editing.value.id ? `编辑: ${editing.value.id}` : '新建 OCR 场景'}>
          <div style={{ display: 'grid', gap: 8, maxWidth: 520 }}>
            <label>
              ID (slug, e.g. medical-x) —{' '}
              <input
                disabled={!!editing.value.builtin}
                value={editing.value.id}
                onInput={(e) =>
                  (editing.value = { ...editing.value!, id: (e.target as HTMLInputElement).value })
                }
              />
            </label>
            <label>
              名称 —{' '}
              <input
                value={editing.value.name}
                onInput={(e) =>
                  (editing.value = { ...editing.value!, name: (e.target as HTMLInputElement).value })
                }
              />
            </label>
            <label>
              说明 —{' '}
              <input
                value={editing.value.description}
                onInput={(e) =>
                  (editing.value = {
                    ...editing.value!,
                    description: (e.target as HTMLInputElement).value,
                  })
                }
              />
            </label>
            <label>
              语言 (chi_sim+eng / chi_sim / eng) —{' '}
              <input
                value={editing.value.languages}
                onInput={(e) =>
                  (editing.value = {
                    ...editing.value!,
                    languages: (e.target as HTMLInputElement).value,
                  })
                }
              />
            </label>
            <label>
              DPI [72-1200] —{' '}
              <input
                type="number"
                value={editing.value.dpi}
                min={72}
                max={1200}
                onInput={(e) =>
                  (editing.value = {
                    ...editing.value!,
                    dpi: parseInt((e.target as HTMLInputElement).value, 10) || 300,
                  })
                }
              />
            </label>
            <label>
              标签 (逗号分隔) —{' '}
              <input
                value={editing.value.tags.join(',')}
                onInput={(e) =>
                  (editing.value = {
                    ...editing.value!,
                    tags: (e.target as HTMLInputElement).value
                      .split(',')
                      .map((s) => s.trim())
                      .filter(Boolean),
                  })
                }
              />
            </label>
            <div style={{ display: 'flex', gap: 8 }}>
              <Button variant="primary" disabled={saving.value} onClick={() => void onSave()}>
                {saving.value ? '保存中...' : '保存'}
              </Button>
              <Button variant="ghost" onClick={() => (editing.value = null)}>
                取消
              </Button>
            </div>
          </div>
        </Section>
      )}
    </div>
  );
}

// ============ Member Panel ============

function MemberPanel(): JSX.Element {
  useEffect(() => {
    void loadMemberState();
    void loadSettingsLocks();
  }, []);

  const m = memberState.value;
  const l = settingsLocks.value;

  return (
    <div>
      <Section title="会员状态" desc="对接 cloud accounts; CLI: attune login <email>">
        {!m && <p style={{ color: 'var(--color-text-secondary)' }}>加载中...</p>}
        {m && (
          <>
            <p>
              状态:{' '}
              <strong style={{ color: m.is_paid ? 'var(--color-accent)' : 'var(--color-text)' }}>
                {m.kind === 'paid' ? '付费会员' : m.kind === 'free' ? '免费会员' : '未登录'}
              </strong>
            </p>
            {m.account_id && <p>账号: <code>{m.account_id}</code></p>}
            {m.license_id && <p>License: <code>{m.license_id}</code></p>}
            {m.is_logged_in && (
              <Button
                variant="ghost"
                onClick={async () => {
                  if (await memberLogout()) toast('success', '已登出');
                  else toast('error', '登出失败');
                }}
              >
                登出
              </Button>
            )}
          </>
        )}
      </Section>

      <Section title="设置锁定矩阵" desc="GET /api/v1/member/locks — 决定 UI 哪些字段可改">
        {!l && <p style={{ color: 'var(--color-text-secondary)' }}>加载中...</p>}
        {l && (
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 'var(--text-sm)' }}>
            <tbody>
              {(
                [
                  ['vault_password', 'Vault 主密码'],
                  ['local_folder_links', '本地知识库目录'],
                  ['cloud_llm', '云端大模型 API'],
                  ['plugin_install', '插件装载'],
                  ['plugin_uninstall', '插件卸载'],
                  ['ocr_profiles', 'OCR 场景预设'],
                ] as const
              ).map(([k, label]) => (
                <tr key={k} style={{ borderBottom: '1px solid var(--color-border-subtle)' }}>
                  <td style={{ padding: 6 }}>{label}</td>
                  <td style={{ padding: 6, fontFamily: 'monospace' }}>{k}</td>
                  <td style={{ padding: 6 }}>
                    {l[k] === 'locked' ? (
                      <span style={{ color: 'var(--color-warning)' }}>🔒 locked</span>
                    ) : (
                      <span style={{ color: 'var(--color-accent)' }}>✏️ editable</span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Section>
    </div>
  );
}

// ============ Folder Links Panel (注入 DataPanel 用, 这里也单独 export) ============

export function FolderLinksSection(): JSX.Element {
  useEffect(() => {
    void loadFolderLinks();
  }, []);
  const links = folderLinks.value;
  return (
    <Section
      title="本地知识库目录"
      desc="GET /api/v1/folder-links (只读). 写入用 CLI: attune link-folder <path>"
    >
      {links.length === 0 && (
        <p style={{ color: 'var(--color-text-secondary)' }}>
          尚未关联任何本地目录. 在终端运行 <code>attune link-folder /path/to/docs</code> 添加.
        </p>
      )}
      {links.length > 0 && (
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 'var(--text-sm)' }}>
          <thead>
            <tr style={{ textAlign: 'left', borderBottom: '1px solid var(--color-border)' }}>
              <th style={{ padding: 8 }}>路径</th>
              <th style={{ padding: 8 }}>Project</th>
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
                <td style={{ padding: 8 }}>{fl.project_id ?? 'default'}</td>
                <td style={{ padding: 8 }}>{fl.added_at ?? '—'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Section>
  );
}
