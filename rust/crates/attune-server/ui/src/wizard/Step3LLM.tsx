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

  // 云端 API 表单
  const [provider, setProvider] = useState<string>('openai');
  const [apiKey, setApiKey] = useState('');
  const [endpoint, setEndpoint] = useState('https://api.openai.com/v1');
  const [cloudModel, setCloudModel] = useState('gpt-4o-mini');
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);

  async function scanOllama() {
    setOllamaStatus('checking');
    setScanning(true);
    try {
      const d = await api.get<Diagnostics>('/status/diagnostics');
      if (d.ollama_status === 'ready') {
        setOllamaStatus('ready');
        setOllamaModels(d.ollama_models ?? []);
      } else {
        setOllamaStatus('missing');
      }
    } catch {
      setOllamaStatus('missing');
    } finally {
      setScanning(false);
    }
  }

  useEffect(() => {
    void scanOllama();
  }, []);

  async function testCloudConnection() {
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
    onUpdate({ llmMode: 'ollama' });
    try {
      await api.patch('/settings', {
        llm: { endpoint: null, api_key: '', model: ollamaModels[0] ?? 'qwen2.5:3b' },
      });
    } catch {
      /* 保存失败不阻塞流程 */
    }
    onContinue();
  }

  async function selectCloud() {
    if (!apiKey || !endpoint || !cloudModel) {
      toast('error', '请填完 API Key / Endpoint / Model');
      return;
    }
    onUpdate({ llmMode: 'cloud' });
    try {
      await api.patch('/settings', {
        llm: { endpoint, api_key: apiKey, model: cloudModel, provider },
      });
    } catch {
      /* 保存失败不阻塞 */
    }
    onContinue();
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
          gridTemplateColumns: '1fr 1fr 1fr',
          gap: 'var(--space-3)',
        }}
      >
        {/* Ollama 卡片 */}
        <Card
          selected={ctx.llmMode === 'ollama'}
          onClick={ollamaStatus === 'ready' ? selectOllama : undefined}
          disabled={ollamaStatus !== 'ready'}
        >
          <CardHeader
            icon="🟢"
            title={t('wizard.llm.ollama.title')}
            tag={t('wizard.llm.ollama.tag')}
          />
          <div style={{ fontSize: 'var(--text-sm)', minHeight: 60 }}>
            {scanning && (
              <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-2)' }}>
                <span className="spinner" />
                {t('wizard.llm.ollama.detecting')}
              </div>
            )}
            {!scanning && ollamaStatus === 'ready' && (
              <div style={{ color: 'var(--color-success)' }}>
                {t('wizard.llm.ollama.found', { models: ollamaModels.length })}
              </div>
            )}
            {!scanning && ollamaStatus === 'missing' && (
              <div>
                <div style={{ color: 'var(--color-warning)', marginBottom: 'var(--space-2)' }}>
                  {t('wizard.llm.ollama.missing')}
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

        {/* 云端 API 卡片 */}
        <Card
          selected={ctx.llmMode === 'cloud'}
        >
          <CardHeader
            icon="☁"
            title={t('wizard.llm.cloud.title')}
            tag={t('wizard.llm.cloud.tag')}
          />
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
            <select
              value={provider}
              onChange={(e) => {
                setProvider(e.currentTarget.value);
                // 预填常见 provider endpoint
                const presets: Record<string, { endpoint: string; model: string }> = {
                  openai: { endpoint: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
                  anthropic: { endpoint: 'https://api.anthropic.com/v1', model: 'claude-3-5-sonnet-20241022' },
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
              <option value="openai">OpenAI</option>
              <option value="anthropic">Anthropic</option>
              <option value="deepseek">DeepSeek</option>
              <option value="qwen">Qwen (阿里)</option>
              <option value="custom">自定义</option>
            </select>
            <Input
              type="password"
              placeholder="API Key"
              value={apiKey}
              onInput={(e) => setApiKey(e.currentTarget.value)}
            />
            <Input
              type="text"
              placeholder="Model"
              value={cloudModel}
              onInput={(e) => setCloudModel(e.currentTarget.value)}
            />
            <Button
              size="sm"
              variant="secondary"
              onClick={testCloudConnection}
              loading={testing}
              disabled={!apiKey}
            >
              {t('wizard.llm.cloud.test')}
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
              disabled={!apiKey || testResult?.startsWith('✗')}
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
  children,
}: {
  selected?: boolean;
  onClick?: () => void;
  disabled?: boolean;
  children: JSX.Element | JSX.Element[];
}): JSX.Element {
  return (
    <div
      onClick={disabled ? undefined : onClick}
      className="interactive"
      style={{
        padding: 'var(--space-4)',
        background: 'var(--color-surface)',
        border: `2px solid ${selected ? 'var(--color-accent)' : 'var(--color-border)'}`,
        borderRadius: 'var(--radius-lg)',
        cursor: disabled ? 'not-allowed' : onClick ? 'pointer' : 'default',
        opacity: disabled ? 0.6 : 1,
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-3)',
        minHeight: 200,
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
