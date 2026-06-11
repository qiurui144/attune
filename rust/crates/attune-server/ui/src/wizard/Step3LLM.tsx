/** Wizard Step 3 · 选择 AI 大脑 */

import type { JSX } from 'preact';
import { useState, useEffect } from 'preact/hooks';
import { Button, Input, Tooltip, LocalModelReadiness } from '../components';
import { t } from '../i18n';
import { api } from '../store/api';
import { toast } from '../components/Toast';
import type { WizardContext } from './types';

type OllamaStatus = 'checking' | 'ready' | 'missing';

type Diagnostics = {
  ollama_status?: string;
  ollama_models?: string[];
  hardware?: {
    form_factor?: 'laptop' | 'k3' | 'server' | 'unknown';
    prefers_local_llm?: boolean;
    recommended_summary_model?: string;
  };
};

type LmStudioProbeResponse = {
  found: boolean;
  endpoint?: string | null;
  models: string[];
  download_url: string;
};

type AiStackGate = {
  hardware?: {
    tier?: 'unsupported' | 'low' | 'mid' | 'high' | 'flagship';
    supported?: boolean;
  };
};

type ProbeK3Response = {
  found: boolean;
  endpoint?: string | null;
  checked: string[];
};

export type Step3Props = {
  ctx: WizardContext;
  onUpdate: (partial: Partial<WizardContext>) => void;
  onContinue: () => void;
};

export function Step3LLM({ ctx, onUpdate, onContinue }: Step3Props): JSX.Element {
  const [ollamaStatus, setOllamaStatus] = useState<OllamaStatus>('checking');
  const [ollamaModels, setOllamaModels] = useState<string[]>([]);
  // 形态分裂：K3 一体机优先本地 Ollama；Laptop/Server 默认远端 token
  const [prefersLocal, setPrefersLocal] = useState<boolean>(false);
  const [localChatAllowed, setLocalChatAllowed] = useState<boolean>(false);
  const [localBlockReason, setLocalBlockReason] = useState<string>(t('wizard.llm.block.default_reason'));
  const [k3Endpoint, setK3Endpoint] = useState('http://192.168.100.166:8080/v1');
  const [k3Detecting, setK3Detecting] = useState(false);
  const [k3DetectResult, setK3DetectResult] = useState<string | null>(null);
  // 本地模型一键就绪：目标 chat 模型 = 硬件推荐 (fallback qwen2.5:3b)。
  const [ollamaTargetModel, setOllamaTargetModel] = useState('qwen2.5:3b');
  // LM Studio (OpenAI 兼容 :1234) 自动探测
  const [lmStudio, setLmStudio] = useState<LmStudioProbeResponse | null>(null);
  const [lmStudioProbing, setLmStudioProbing] = useState(false);

  // 云端 API 表单
  // Default: attune-pro membership — 登录即用，token 配额由 attune 计费追踪
  // 用户拍板（2026-05-01）：attune Pro 会员 + 用户已有 BYOK 支撑；
  // 不预设第三方 free API tier（避免误导）；本地 LLM 暂时不主推（研发成本高）
  const [provider, setProvider] = useState<string>('attune-pro');
  const [apiKey, setApiKey] = useState('');
  const [endpoint, setEndpoint] = useState('https://gateway.engi-stack.com/v1');
  const [cloudModel, setCloudModel] = useState('auto');
  const [memberLoggingIn, setMemberLoggingIn] = useState(false);
  const [memberReady, setMemberReady] = useState(false);
  const [testing, setTesting] = useState(false);
  // 默认隐藏 Ollama / K3 (这两个面向高级用户). 用户主动展开"其他选项"才看到.
  const [showAdvancedProviders, setShowAdvancedProviders] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);

  async function scanOllama() {
    setOllamaStatus('checking');
    try {
      const [d, gate] = await Promise.all([
        api.get<Diagnostics>('/status/diagnostics'),
        api.get<AiStackGate>('/ai_stack'),
      ]);
      if (d.ollama_status === 'ready') {
        setOllamaStatus('ready');
        setOllamaModels(d.ollama_models ?? []);
      } else {
        setOllamaStatus('missing');
      }
      // 读形态：K3 → 主推 Ollama；Laptop/其他 → 主推云端
      setPrefersLocal(d.hardware?.prefers_local_llm === true);
      // 本地一键拉取的目标模型 = 硬件推荐（弱机自动落到轻量模型）
      if (d.hardware?.recommended_summary_model) {
        setOllamaTargetModel(d.hardware.recommended_summary_model);
      }

      const tier = gate.hardware?.tier ?? 'unsupported';
      const allow = gate.hardware?.supported === true && (tier === 'high' || tier === 'flagship');
      setLocalChatAllowed(allow);
      if (!allow) {
        setLocalBlockReason(t('wizard.llm.block.tier_reason'));
      }
    } catch {
      setOllamaStatus('missing');
      setLocalChatAllowed(false);
      setLocalBlockReason(t('wizard.llm.block.detect_failed'));
    }
  }

  useEffect(() => {
    void scanOllama();
    void autoDetectK3(true);
    void probeLmStudio(true);
  }, []);

  async function probeLmStudio(silent = false) {
    setLmStudioProbing(true);
    try {
      const res = await api.get<LmStudioProbeResponse>('/lmstudio/probe');
      setLmStudio(res);
      if (res.found && !silent) {
        toast('success', t('wizard.llm.lmstudio.detected', { count: res.models.length }));
      } else if (!res.found && !silent) {
        toast('error', t('wizard.llm.lmstudio.not_found'));
      }
    } catch {
      setLmStudio(null);
    } finally {
      setLmStudioProbing(false);
    }
  }

  async function selectLmStudio() {
    if (!lmStudio?.found || !lmStudio.endpoint) {
      toast('error', t('wizard.llm.lmstudio.not_found'));
      return;
    }
    const lmModel = lmStudio.models[0] ?? 'auto';
    onUpdate({ llmMode: 'cloud' });
    try {
      // LM Studio = 本机 OpenAI 兼容 server，按 openai_compat 接入（无需 api_key）。
      await api.patch('/settings', {
        llm: {
          endpoint: lmStudio.endpoint,
          api_key: '',
          model: lmModel,
          provider: 'openai_compat',
        },
      });
    } catch {
      /* 保存失败不阻塞 */
    }
    onContinue();
  }

  async function autoDetectK3(silent = false) {
    setK3Detecting(true);
    if (!silent) {
      setK3DetectResult(null);
    }
    try {
      const res = await api.post<ProbeK3Response>('/llm/probe-k3', {});
      if (res.found && res.endpoint) {
        setK3Endpoint(res.endpoint);
        const msg = t('wizard.llm.k3.detected', { endpoint: res.endpoint });
        setK3DetectResult(msg);
        if (!silent) {
          toast('success', msg);
        }
      } else {
        const msg = t('wizard.llm.k3.not_found');
        setK3DetectResult(msg);
        if (!silent) {
          toast('error', msg);
        }
      }
    } catch (e) {
      const msg = t('wizard.llm.k3.detect_failed', { message: e instanceof Error ? e.message : String(e) });
      setK3DetectResult(msg);
      if (!silent) {
        toast('error', msg);
      }
    } finally {
      setK3Detecting(false);
    }
  }

  async function testCloudConnection() {
    if (provider === 'attune-pro') {
      const ok = await loginMember();
      setTestResult(ok ? t('wizard.llm.member.verify_pass') : t('wizard.llm.member.verify_fail'));
      return;
    }
    setTesting(true);
    setTestResult(null);
    try {
      const res = await api.post<{ ok: boolean; latency_ms?: number; error?: string }>(
        '/llm/test',
        { endpoint, api_key: apiKey, model: cloudModel },
      );
      if (res.ok) {
        setTestResult(`✓ ${res.latency_ms ?? '?'}ms`);
      } else {
        setTestResult(`✗ ${res.error ?? 'unknown error'}`);
      }
    } catch (e) {
      setTestResult(`✗ ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setTesting(false);
    }
  }

  async function selectOllama() {
    if (!localChatAllowed) {
      toast('error', localBlockReason);
      return;
    }
    onUpdate({ llmMode: 'ollama' });
    try {
      await api.patch('/settings', {
        llm: { endpoint: null, api_key: '', model: ollamaModels[0] ?? null },
      });
    } catch {
      /* 保存失败不阻塞流程 */
    }
    onContinue();
  }

  async function selectK3() {
    if (!k3Endpoint.trim()) {
      toast('error', t('wizard.llm.k3.need_endpoint'));
      return;
    }
    onUpdate({ llmMode: 'k3' });
    try {
      await api.patch('/settings', {
        llm: {
          endpoint: k3Endpoint.trim(),
          api_key: '',
          model: 'auto',
          provider: 'openai_compat',
        },
      });
    } catch {
      /* 保存失败不阻塞流程 */
    }
    onContinue();
  }

  async function selectCloud() {
    if (!endpoint || !cloudModel) {
      toast('error', t('wizard.llm.cloud.need_fields'));
      return;
    }

    if (provider === 'custom' && !apiKey) {
      toast('error', t('wizard.llm.cloud.need_custom'));
      return;
    }

    if (provider === 'attune-pro' && !memberReady) {
      if (!ctx.memberEmail || !ctx.memberPassword) {
        toast('error', t('wizard.llm.member.need_step2'));
        return;
      }
      const ok = await loginMember();
      if (!ok) return;
    }

    if (provider !== 'attune-pro' && !apiKey) {
      toast('error', t('wizard.llm.cloud.need_apikey'));
      return;
    }
    onUpdate({ llmMode: 'cloud' });
    try {
      await api.patch('/settings', {
        llm: {
          endpoint,
          api_key: provider === 'attune-pro' ? '' : apiKey,
          model: cloudModel,
          provider,
        },
      });
    } catch {
      /* 保存失败不阻塞 */
    }
    onContinue();
  }

  async function loginMember(): Promise<boolean> {
    if (!ctx.memberEmail || !ctx.memberPassword) {
      toast('error', t('wizard.llm.member.step2_missing'));
      return false;
    }
    setMemberLoggingIn(true);
    try {
      await api.post('/member/login-password', {
        email: ctx.memberEmail,
        password: ctx.memberPassword,
        license_code: ctx.memberLicenseCode?.trim() || null,
      });
      setMemberReady(true);
      toast('success', t('wizard.llm.member.login_ok'));
      return true;
    } catch (e) {
      setMemberReady(false);
      toast('error', t('wizard.llm.member.login_fail', { message: e instanceof Error ? e.message : String(e) }));
      return false;
    } finally {
      setMemberLoggingIn(false);
    }
  }

  function selectSkip() {
    onUpdate({ llmMode: 'skip' });
    onContinue();
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-5)' }}>
      <h2
        style={{
          fontSize: 'var(--text-xl)',
          fontWeight: 600,
          margin: 0,
          display: 'flex',
          alignItems: 'center',
        }}
      >
        {t('wizard.llm.heading')}
        <Tooltip text={t('wizard.help.llm_provider')} />
      </h2>

      {/* 默认显示: 云端 + 暂不配置 两张 */}
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(280px, 1fr))',
          gap: 'var(--space-4)',
          alignItems: 'start',
        }}
      >
        {/* Ollama + K3 默认隐藏, 用户点 "其他选项" 才显示 (面向高级用户) */}
        {showAdvancedProviders && (
          <>
            {/* Ollama 卡片 */}
        <Card
          selected={ctx.llmMode === 'ollama'}
          onClick={ollamaStatus === 'ready' && localChatAllowed ? selectOllama : undefined}
          disabled={ollamaStatus !== 'ready' || !localChatAllowed}
          recommended={prefersLocal}
        >
          <CardHeader
            icon="🟢"
            title={t('wizard.llm.ollama.title')}
            tag={prefersLocal ? '★ ' + t('wizard.llm.ollama.tag') : t('wizard.llm.ollama.tag')}
          />
          <div style={{ fontSize: 'var(--text-sm)', minHeight: 56 }}>
            {/* 本地模型一键就绪：三态 + 一键安装/拉取（替代过去让用户复制 curl 命令）。
                model 用硬件推荐的本地 chat 模型；就绪后允许选 Ollama。 */}
            <LocalModelReadiness
              model={ollamaTargetModel}
              onReadyChange={(ready) => setOllamaStatus(ready ? 'ready' : 'missing')}
            />
            {!localChatAllowed && (
              <div style={{ color: 'var(--color-warning)', marginTop: 'var(--space-2)' }}>
                {localBlockReason}
              </div>
            )}
          </div>
        </Card>

        {/* K3 第三方设备卡片 */}
        <Card selected={ctx.llmMode === 'k3'}>
          <CardHeader icon="🧩" title={t('wizard.llm.k3.card_title')} tag={t('wizard.llm.k3.card_tag')} />
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
            <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
              {t('wizard.llm.k3.card_desc')}
            </div>
            <Button size="sm" variant="secondary" onClick={() => void autoDetectK3()} loading={k3Detecting}>
              {t('wizard.llm.k3.detect_btn')}
            </Button>
            <Input
              type="text"
              placeholder={t('wizard.llm.k3.endpoint_placeholder')}
              value={k3Endpoint}
              onInput={(e) => setK3Endpoint(e.currentTarget.value)}
            />
            {k3DetectResult && (
              <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
                {k3DetectResult}
              </div>
            )}
            <Button size="sm" variant="primary" onClick={selectK3} disabled={!k3Endpoint.trim()}>
              {t('wizard.llm.k3.use_btn')}
            </Button>
          </div>
        </Card>

        {/* LM Studio 卡片：自动探测本机 :1234 OpenAI 兼容 server */}
        <Card selected={false}>
          <CardHeader icon="🖥" title={t('wizard.llm.lmstudio.title')} tag={t('wizard.llm.lmstudio.tag')} />
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
            <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
              {t('wizard.llm.lmstudio.desc')}
            </div>
            <Button size="sm" variant="secondary" onClick={() => void probeLmStudio()} loading={lmStudioProbing}>
              {t('wizard.llm.lmstudio.detect_btn')}
            </Button>
            {lmStudio?.found && (
              <>
                <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-success)' }}>
                  {t('wizard.llm.lmstudio.found', { count: lmStudio.models.length })}
                </div>
                <Button size="sm" variant="primary" onClick={selectLmStudio}>
                  {t('wizard.llm.lmstudio.use_btn')}
                </Button>
              </>
            )}
            {lmStudio && !lmStudio.found && (
              <button
                type="button"
                onClick={() => window.open(lmStudio.download_url, '_blank', 'noopener')}
                style={{
                  background: 'transparent', border: 'none', color: 'var(--color-accent)',
                  fontSize: 'var(--text-xs)', cursor: 'pointer', padding: 0, textAlign: 'left',
                }}
              >
                {t('wizard.llm.lmstudio.download_link')}
              </button>
            )}
          </div>
        </Card>
          </>
        )}

        {/* 云端 API 卡片 (默认显示) */}
        <Card
          selected={ctx.llmMode === 'cloud'}
          recommended={!prefersLocal}
        >
          <CardHeader
            icon="☁"
            title={t('wizard.llm.cloud.title')}
            tag={!prefersLocal ? '★ ' + t('wizard.llm.cloud.tag') : t('wizard.llm.cloud.tag')}
          />
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
            <select
              value={provider}
              onChange={(e) => {
                setProvider(e.currentTarget.value);
                // 预填常见 provider endpoint
                // 设计（2026-05-01 用户拍板，澄清版）：
                //   - 笔电暂时不走本地 LLM（研发成本高，云端更准确，等本地解决不了再上）
                //   - 主推 attune Pro 会员（登录即用，token 配额由 attune 计费跟踪）
                //   - 用户已有的 web 会员（ChatGPT Plus / Claude Pro / Gemini Advanced）→ 走 BYOK API key
                //   - 不预设第三方 "free API tier"（避免误导，用户的"免费"指浏览器 web 会话）
                const presets: Record<string, { endpoint: string; model: string }> = {
                  // ── ★ 主推：attune Pro 会员 gateway ──
                  'attune-pro': { endpoint: 'https://gateway.engi-stack.com/v1', model: 'auto' },
                  // ── BYOK：用户已有付费会员的 API key ──
                  openai: { endpoint: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
                  anthropic: { endpoint: 'https://api.anthropic.com/v1', model: 'claude-3-5-sonnet-20241022' },
                  gemini: { endpoint: 'https://generativelanguage.googleapis.com/v1beta/openai', model: 'gemini-2.0-flash' },
                  deepseek: { endpoint: 'https://api.deepseek.com/v1', model: 'deepseek-chat' },
                  qwen: { endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen-plus' },
                };
                const preset = presets[e.currentTarget.value];
                if (preset) {
                  setEndpoint(preset.endpoint);
                  setCloudModel(preset.model);
                }
              }}
              style={{
                height: 'var(--btn-h-sm)',
                padding: '0 var(--space-2)',
                background: 'var(--color-surface)',
                border: '1px solid var(--color-border)',
                borderRadius: 'var(--radius-sm)',
                fontSize: 'var(--text-sm)',
              }}
            >
              <optgroup label={t('wizard.llm.cloud.optgroup_recommended')}>
                <option value="attune-pro">{t('wizard.llm.cloud.opt_attune_pro')}</option>
              </optgroup>
              <optgroup label={t('wizard.llm.cloud.optgroup_byok')}>
                <option value="openai">{t('wizard.llm.cloud.opt_openai')}</option>
                <option value="anthropic">{t('wizard.llm.cloud.opt_anthropic')}</option>
                <option value="gemini">{t('wizard.llm.cloud.opt_gemini')}</option>
                <option value="deepseek">{t('wizard.llm.cloud.opt_deepseek')}</option>
                <option value="qwen">{t('wizard.llm.cloud.opt_qwen')}</option>
                <option value="custom">{t('wizard.llm.cloud.opt_custom')}</option>
              </optgroup>
            </select>
            {provider === 'attune-pro' && (
              <>
                <div style={{ display: 'grid', gap: 'var(--space-2)' }}>
                  <StatusChip
                    tone={memberReady ? 'success' : 'muted'}
                    title={t('wizard.llm.member.chip_title')}
                    value={memberReady ? t('wizard.llm.member.chip_logged_in') : t('wizard.llm.member.chip_pending')}
                  />
                  <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
                    {t('wizard.llm.member.input_once_hint')}
                  </div>
                </div>
                <Input
                  type="text"
                  placeholder={t('wizard.llm.cloud.endpoint_default_placeholder')}
                  value={endpoint}
                  onInput={(e) => setEndpoint(e.currentTarget.value)}
                />
                <Button
                  size="sm"
                  variant={memberReady ? 'secondary' : 'primary'}
                  onClick={loginMember}
                  loading={memberLoggingIn}
                  disabled={!ctx.memberEmail || !ctx.memberPassword}
                >
                  {memberReady ? t('wizard.llm.member.logged_in_btn') : t('wizard.llm.member.login_btn')}
                </Button>
              </>
            )}
            {provider !== 'attune-pro' && (
              <>
                <Input
                  type="text"
                  placeholder={provider === 'custom' ? t('wizard.llm.cloud.custom_url_placeholder') : t('wizard.llm.cloud.endpoint_url_placeholder')}
                  value={endpoint}
                  onInput={(e) => setEndpoint(e.currentTarget.value)}
                />
                <Input
                  type="password"
                  placeholder={provider === 'custom' ? t('wizard.llm.cloud.custom_token_placeholder') : t('wizard.llm.cloud.apikey_placeholder')}
                  value={apiKey}
                  onInput={(e) => setApiKey(e.currentTarget.value)}
                />
              </>
            )}
            <Input
              type="text"
              placeholder={t('wizard.llm.cloud.model_placeholder')}
              value={cloudModel}
              onInput={(e) => setCloudModel(e.currentTarget.value)}
            />
            <Button
              size="sm"
              variant="secondary"
              onClick={testCloudConnection}
              loading={testing}
              disabled={provider === 'attune-pro'
                ? (!ctx.memberEmail || !ctx.memberPassword)
                : (!apiKey || !endpoint)}
            >
              {provider === 'attune-pro' ? t('wizard.llm.cloud.verify_login') : t('wizard.llm.cloud.test')}
            </Button>
            {testResult && (
              <div
                style={{
                  fontSize: 'var(--text-xs)',
                  color: testResult.startsWith('✓')
                    ? 'var(--color-success)'
                    : 'var(--color-error)',
                }}
              >
                {testResult}
              </div>
            )}
            <Button
              size="sm"
              variant="primary"
              onClick={selectCloud}
              disabled={
                (provider === 'attune-pro'
                  ? (!ctx.memberEmail || !ctx.memberPassword)
                  : (!apiKey || !endpoint))
                || testResult?.startsWith('✗')
              }
            >
              {t('wizard.llm.cloud.use_btn')}
            </Button>
          </div>
        </Card>

        {/* 跳过卡片 */}
        <Card selected={ctx.llmMode === 'skip'} onClick={selectSkip}>
          <CardHeader
            icon="💤"
            title={t('wizard.llm.skip.title')}
            tag={t('wizard.llm.skip.tag')}
          />
          <p
            style={{
              fontSize: 'var(--text-sm)',
              color: 'var(--color-text-secondary)',
              margin: 0,
            }}
          >
            {t('wizard.llm.skip.desc')}
          </p>
        </Card>
      </div>

      {/* "其他选项" toggle — 默认折叠, 展开后显示 Ollama + K3 */}
      {!showAdvancedProviders && (
        <button
          type="button"
          onClick={() => setShowAdvancedProviders(true)}
          style={{
            background: 'transparent',
            border: 'none',
            color: 'var(--color-text-secondary)',
            fontSize: 'var(--text-sm)',
            cursor: 'pointer',
            padding: 'var(--space-2) 0',
            alignSelf: 'flex-start',
          }}
        >
          {t('wizard.llm.advanced_toggle')}
        </button>
      )}
    </div>
  );
}

// ─── 卡片容器 ──────────────────────────────────────────────
function Card({
  selected,
  onClick,
  disabled,
  recommended,
  children,
}: {
  selected?: boolean;
  onClick?: () => void;
  disabled?: boolean;
  recommended?: boolean;
  children: JSX.Element | JSX.Element[];
}): JSX.Element {
  // recommended 卡用更亮的边框色（即使未 selected 也提示）
  const borderColor = selected
    ? 'var(--color-accent)'
    : recommended
    ? 'var(--color-accent)'
    : 'var(--color-border)';
  return (
    <div
      onClick={disabled ? undefined : onClick}
      className="interactive"
      style={{
        padding: 'var(--space-4)',
        background: 'var(--color-surface)',
        border: `${recommended && !selected ? '2px dashed' : '2px solid'} ${borderColor}`,
        borderRadius: 'var(--radius-lg)',
        cursor: disabled ? 'not-allowed' : onClick ? 'pointer' : 'default',
        opacity: disabled ? 0.6 : 1,
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-3)',
        minHeight: 0,
      }}
    >
      {children}
    </div>
  );
}

function CardHeader({
  icon,
  title,
  tag,
}: {
  icon: string;
  title: string;
  tag: string;
}): JSX.Element {
  return (
    <div>
      <div style={{ fontSize: 24, marginBottom: 'var(--space-1)' }} aria-hidden="true">
        {icon}
      </div>
      <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>{title}</h3>
      <span
        style={{
          display: 'inline-block',
          fontSize: 'var(--text-xs)',
          color: 'var(--color-accent)',
          marginTop: 'var(--space-1)',
        }}
      >
        {tag}
      </span>
    </div>
  );
}

function StatusChip({
  title,
  value,
  tone,
}: {
  title: string;
  value: string;
  tone: 'accent' | 'success' | 'muted';
}): JSX.Element {
  const palette =
    tone === 'success'
      ? { border: 'var(--color-success)', bg: 'rgba(34, 197, 94, 0.08)', fg: 'var(--color-success)' }
      : tone === 'accent'
        ? { border: 'var(--color-accent)', bg: 'rgba(14, 165, 233, 0.08)', fg: 'var(--color-accent)' }
        : { border: 'var(--color-border)', bg: 'var(--color-bg)', fg: 'var(--color-text-secondary)' };

  return (
    <div
      style={{
        border: `1px solid ${palette.border}`,
        background: palette.bg,
        borderRadius: 'var(--radius-md)',
        padding: 'var(--space-3)',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-1)',
      }}
    >
      <div style={{ fontSize: 'var(--text-xs)', color: palette.fg, fontWeight: 600 }}>{title}</div>
      <div style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text)' }}>{value}</div>
    </div>
  );
}
