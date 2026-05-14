/** Wizard Step 3 · 选择 AI 大脑 */

import type { JSX } from 'preact';
import { useState, useEffect } from 'preact/hooks';
import { Button, Input } from '../components';
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
  };
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
  const [scanning, setScanning] = useState(true);
  // 形态分裂：K3 一体机优先本地 Ollama；Laptop/Server 默认远端 token
  const [prefersLocal, setPrefersLocal] = useState<boolean>(false);
  const [localChatAllowed, setLocalChatAllowed] = useState<boolean>(false);
  const [localBlockReason, setLocalBlockReason] = useState<string>('当前硬件规格不建议本地对话');
  const [k3Endpoint, setK3Endpoint] = useState('http://192.168.100.166:8080/v1');
  const [k3Detecting, setK3Detecting] = useState(false);
  const [k3DetectResult, setK3DetectResult] = useState<string | null>(null);

  // 云端 API 表单
  // Default: attune-pro membership — 登录即用，token 配额由 attune 计费追踪
  // 用户拍板（2026-05-01）：attune Pro 会员 + 用户已有 BYOK 支撑；
  // 不预设第三方 free API tier（避免误导）；本地 LLM 暂时不主推（研发成本高）
  const [provider, setProvider] = useState<string>('attune-pro');
  const [apiKey, setApiKey] = useState('');
  const [endpoint, setEndpoint] = useState('https://gateway.attune.ai/v1');
  const [cloudModel, setCloudModel] = useState('auto');
  const [memberLoggingIn, setMemberLoggingIn] = useState(false);
  const [memberReady, setMemberReady] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);

  async function scanOllama() {
    setOllamaStatus('checking');
    setScanning(true);
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

      const tier = gate.hardware?.tier ?? 'unsupported';
      const allow = gate.hardware?.supported === true && (tier === 'high' || tier === 'flagship');
      setLocalChatAllowed(allow);
      if (!allow) {
        setLocalBlockReason('当前硬件规格下默认禁用本地对话，请选择云端或 K3 一体机。');
      }
    } catch {
      setOllamaStatus('missing');
      setLocalChatAllowed(false);
      setLocalBlockReason('硬件检测失败，默认禁用本地对话。');
    } finally {
      setScanning(false);
    }
  }

  useEffect(() => {
    void scanOllama();
    void autoDetectK3(true);
  }, []);

  async function autoDetectK3(silent = false) {
    setK3Detecting(true);
    if (!silent) {
      setK3DetectResult(null);
    }
    try {
      const res = await api.post<ProbeK3Response>('/llm/probe-k3', {});
      if (res.found && res.endpoint) {
        setK3Endpoint(res.endpoint);
        const msg = `已检测到 K3：${res.endpoint}`;
        setK3DetectResult(msg);
        if (!silent) {
          toast('success', msg);
        }
      } else {
        const msg = '未检测到可用 K3，已保留手动输入。';
        setK3DetectResult(msg);
        if (!silent) {
          toast('error', msg);
        }
      }
    } catch (e) {
      const msg = `自动检测失败：${e instanceof Error ? e.message : String(e)}`;
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
      setTestResult(ok ? '✓ 会员登录验证通过' : '✗ 会员登录验证失败');
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
      toast('error', '请填写 K3 地址');
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
      toast('error', '请填完地址和模型名');
      return;
    }

    if (provider === 'custom' && !apiKey) {
      toast('error', '自定义配置需要填写 URL 和 Token');
      return;
    }

    if (provider === 'attune-pro' && !memberReady) {
      if (!ctx.memberEmail || !ctx.memberPassword) {
        toast('error', '请先回到第二步填写会员账号密码');
        return;
      }
      const ok = await loginMember();
      if (!ok) return;
    }

    if (provider !== 'attune-pro' && !apiKey) {
      toast('error', '请填完 API Key / Endpoint / Model');
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
      toast('error', '第二步未填写会员账号密码');
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
      toast('success', '会员账号登录成功');
      return true;
    } catch (e) {
      setMemberReady(false);
      toast('error', `会员登录失败：${e instanceof Error ? e.message : String(e)}`);
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
        }}
      >
        {t('wizard.llm.heading')}
      </h2>

      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))',
          gap: 'var(--space-2)',
        }}
      >
        <StatusChip
          tone={prefersLocal ? 'accent' : 'muted'}
          title="本地"
          value={prefersLocal ? 'K3 / Ollama 推荐' : '默认关闭'}
        />
        <StatusChip
          tone={k3DetectResult?.startsWith('已检测到') ? 'success' : 'muted'}
          title="K3"
          value={k3Detecting ? '自动检测中…' : k3DetectResult ?? '待检测'}
        />
        <StatusChip
          tone={provider === 'attune-pro' ? 'success' : 'muted'}
          title="云端"
          value="账号 / Token 最小输入"
        />
      </div>

      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(280px, 1fr))',
          gap: 'var(--space-4)',
          alignItems: 'start',
        }}
      >
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
            {scanning && (
              <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)' }}>
                <span className="spinner" />
                扫描本机 Ollama
              </div>
            )}
            {!scanning && ollamaStatus === 'ready' && (
              <>
                <div style={{ color: 'var(--color-success)' }}>
                  已发现 {ollamaModels.length} 个模型
                </div>
                {!localChatAllowed && (
                  <div style={{ color: 'var(--color-warning)', marginTop: 'var(--space-2)' }}>
                    {localBlockReason}
                  </div>
                )}
              </>
            )}
            {!scanning && ollamaStatus === 'missing' && (
              <div>
                <div style={{ color: 'var(--color-warning)', marginBottom: 'var(--space-2)' }}>
                  未检测到本地 Ollama
                </div>
                <code
                  style={{
                    display: 'block',
                    padding: 'var(--space-2)',
                    background: 'var(--color-bg)',
                    borderRadius: 'var(--radius-sm)',
                    fontSize: 'var(--text-xs)',
                    fontFamily: 'var(--font-mono)',
                    wordBreak: 'break-all',
                  }}
                  onClick={(e) => {
                    const text = e.currentTarget.textContent ?? '';
                    navigator.clipboard?.writeText(text);
                    toast('success', '已复制到剪贴板');
                  }}
                >
                    curl -fsSL https://ollama.com/install.sh | sh
                </code>
                <button
                  type="button"
                  onClick={scanOllama}
                  style={{
                    marginTop: 'var(--space-2)',
                    fontSize: 'var(--text-xs)',
                    background: 'transparent',
                    border: 'none',
                    color: 'var(--color-accent)',
                    cursor: 'pointer',
                    padding: 0,
                  }}
                >
                  {t('wizard.llm.ollama.rescan')}
                </button>
              </div>
            )}
          </div>
        </Card>

        {/* K3 第三方设备卡片 */}
        <Card selected={ctx.llmMode === 'k3'}>
          <CardHeader icon="🧩" title="第三方设备（K3）" tag="自动扫描局域网" />
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
            <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
              自动读取本机网段并探测可用 K3，未命中时保留手动输入。
            </div>
            <Button size="sm" variant="secondary" onClick={() => void autoDetectK3()} loading={k3Detecting}>
              自动检测 K3
            </Button>
            <Input
              type="text"
              placeholder="K3 URL（如 http://192.168.100.166:8080/v1）"
              value={k3Endpoint}
              onInput={(e) => setK3Endpoint(e.currentTarget.value)}
            />
            {k3DetectResult && (
              <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
                {k3DetectResult}
              </div>
            )}
            <Button size="sm" variant="primary" onClick={selectK3} disabled={!k3Endpoint.trim()}>
              使用 K3
            </Button>
          </div>
        </Card>

        {/* 云端 API 卡片 */}
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
                  'attune-pro': { endpoint: 'https://gateway.attune.ai/v1', model: 'auto' },
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
              <optgroup label="★ 推荐：Attune Pro 会员（登录即用）">
                <option value="attune-pro">Attune Pro Membership（Gateway，token 配额）</option>
              </optgroup>
              <optgroup label="BYOK：用你已有的 API key">
                <option value="openai">OpenAI（ChatGPT Plus/Team 用户）</option>
                <option value="anthropic">Anthropic（Claude Pro 用户）</option>
                <option value="gemini">Gemini（Google AI Studio）</option>
                <option value="deepseek">DeepSeek</option>
                <option value="qwen">Qwen (阿里通义)</option>
                <option value="custom">自定义（OpenAI 兼容）</option>
              </optgroup>
            </select>
            {provider === 'attune-pro' && (
              <>
                <div style={{ display: 'grid', gap: 'var(--space-2)' }}>
                  <StatusChip
                    tone={memberReady ? 'success' : 'muted'}
                    title="会员"
                    value={memberReady ? '已登录' : '待登录'}
                  />
                  <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
                    账号密码只在此处输入一次，后续自动复用。
                  </div>
                </div>
                <Input
                  type="text"
                  placeholder="Gateway URL（默认可直接用）"
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
                  {memberReady ? '会员已登录 ✓' : '登录会员账号'}
                </Button>
              </>
            )}
            {provider !== 'attune-pro' && (
              <>
                <Input
                  type="text"
                  placeholder={provider === 'custom' ? 'URL 地址（OpenAI 兼容）' : 'Endpoint URL'}
                  value={endpoint}
                  onInput={(e) => setEndpoint(e.currentTarget.value)}
                />
                <Input
                  type="password"
                  placeholder={provider === 'custom' ? 'Token / API Key' : 'API Key'}
                  value={apiKey}
                  onInput={(e) => setApiKey(e.currentTarget.value)}
                />
              </>
            )}
            <Input
              type="text"
              placeholder="模型名（默认 auto 自动选择）"
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
              {provider === 'attune-pro' ? '验证登录' : t('wizard.llm.cloud.test')}
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
              使用云端
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
