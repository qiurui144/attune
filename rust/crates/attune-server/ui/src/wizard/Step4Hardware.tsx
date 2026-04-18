/** Wizard Step 4 · 硬件识别 + 模型推荐（最"路由器"的一步） */

import type { JSX } from 'preact';
import { useState, useEffect } from 'preact/hooks';
import { Button } from '../components';
import { t } from '../i18n';
import { api } from '../store/api';
import type { WizardContext } from './types';

type HardwareInfo = {
  os?: string;
  cpu_model?: string;
  gpu_model?: string | null;
  npu_type?: string | null;
  total_ram_gb?: number;
  recommended_chat?: string;
  recommended_embedding?: string;
  recommended_summary?: string;
};

type ScanStep = {
  label: string;
  done: boolean;
};

export type Step4Props = {
  ctx: WizardContext;
  onUpdate: (partial: Partial<WizardContext>) => void;
  onContinue: () => void;
};

export function Step4Hardware({
  ctx: _ctx,
  onUpdate,
  onContinue,
}: Step4Props): JSX.Element {
  const [hw, setHw] = useState<HardwareInfo | null>(null);
  const [scanSteps, setScanSteps] = useState<ScanStep[]>([]);
  const [autoDownload, setAutoDownload] = useState(true);
  const [applying, setApplying] = useState(false);

  useEffect(() => {
    let cancelled = false;
    async function run() {
      // 阶段扫描动画
      const steps: ScanStep[] = [
        { label: '检测 CPU…', done: false },
        { label: '检测 GPU…', done: false },
        { label: '检测 NPU…', done: false },
        { label: '检测 RAM…', done: false },
        { label: '匹配模型…', done: false },
      ];
      setScanSteps([...steps]);

      try {
        const diag = await api.get<HardwareInfo>('/status/diagnostics');
        if (cancelled) return;

        // 每 400ms tick 一阶段，视觉"扫描感"
        for (let i = 0; i < steps.length; i++) {
          await new Promise((r) => setTimeout(r, 400));
          if (cancelled) return;
          steps[i] = { ...steps[i]!, done: true };
          setScanSteps([...steps]);
        }

        // 结果填充
        setHw({
          os: diag.os,
          cpu_model: diag.cpu_model ?? 'Unknown',
          gpu_model: diag.gpu_model ?? null,
          npu_type: diag.npu_type ?? null,
          total_ram_gb: diag.total_ram_gb,
          recommended_chat: diag.recommended_chat ?? 'qwen2.5:3b',
          recommended_embedding: diag.recommended_embedding ?? 'bge-m3',
          recommended_summary: diag.recommended_summary ?? 'qwen2.5:3b',
        });
      } catch {
        // 失败时 fallback
        setHw({
          cpu_model: 'Unknown',
          total_ram_gb: 0,
          recommended_chat: 'qwen2.5:1.5b',
          recommended_embedding: 'bge-m3',
          recommended_summary: 'qwen2.5:1.5b',
        });
      }
    }
    void run();
    return () => {
      cancelled = true;
    };
  }, []);

  async function applyRecommendation() {
    if (!hw) return;
    setApplying(true);
    onUpdate({
      chatModel: hw.recommended_chat ?? null,
      embeddingModel: hw.recommended_embedding ?? null,
      summaryModel: hw.recommended_summary ?? null,
    });

    try {
      await api.patch('/settings', {
        embedding: { model: hw.recommended_embedding },
        summary_model: hw.recommended_summary,
      });
    } catch {
      /* 保存失败不阻塞 */
    }

    // 后台触发模型下载（可选）
    if (autoDownload && hw.recommended_chat) {
      try {
        await api.post('/models/pull', { model: hw.recommended_chat });
      } catch {
        /* 下载失败只是后台任务问题，不阻塞 wizard */
      }
    }

    onContinue();
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-5)' }}>
      <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0 }}>
        {t('wizard.hw.heading')}
      </h2>

      {/* 扫描进度 */}
      <div
        style={{
          background: 'var(--color-bg)',
          borderRadius: 'var(--radius-md)',
          padding: 'var(--space-4)',
          fontFamily: 'var(--font-mono)',
          fontSize: 'var(--text-sm)',
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-1)',
        }}
      >
        {scanSteps.map((s, i) => (
          <div
            key={i}
            style={{
              color: s.done ? 'var(--color-success)' : 'var(--color-text-secondary)',
              opacity: s.done ? 1 : 0.6,
              transition: 'all var(--duration-base) var(--ease-out)',
            }}
          >
            {s.done ? '✓' : '·'} {s.label}
          </div>
        ))}
      </div>

      {/* 识别结果 */}
      {hw && (
        <div
          className="fade-in"
          style={{
            padding: 'var(--space-4)',
            background: 'var(--color-surface)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-md)',
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--space-2)',
            fontSize: 'var(--text-sm)',
          }}
        >
          <Row label="CPU" value={hw.cpu_model ?? '—'} />
          <Row label="GPU" value={hw.gpu_model ?? '纯 CPU 模式'} />
          <Row label="NPU" value={hw.npu_type ?? '—'} />
          <Row label="RAM" value={`${hw.total_ram_gb ?? 0} GB`} />
        </div>
      )}

      {/* 模型推荐 */}
      {hw && (
        <div className="fade-slide-in" style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
          <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
            {t('wizard.hw.recommend')}
          </h3>
          <ModelRow icon="💬" label="Chat" model={hw.recommended_chat ?? '—'} />
          <ModelRow icon="🧮" label="Embedding" model={hw.recommended_embedding ?? '—'} />
          <ModelRow icon="📄" label="Summary" model={hw.recommended_summary ?? '—'} />

          <label
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 'var(--space-2)',
              fontSize: 'var(--text-sm)',
              color: 'var(--color-text-secondary)',
              cursor: 'pointer',
              marginTop: 'var(--space-2)',
            }}
          >
            <input
              type="checkbox"
              checked={autoDownload}
              onChange={(e) => setAutoDownload(e.currentTarget.checked)}
            />
            {t('wizard.hw.auto_download')}
          </label>
        </div>
      )}

      <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
        <Button
          variant="primary"
          size="lg"
          loading={applying}
          disabled={!hw}
          onClick={applyRecommendation}
        >
          {t('wizard.hw.apply')} →
        </Button>
      </div>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <div style={{ display: 'flex', gap: 'var(--space-3)' }}>
      <span
        style={{
          width: 60,
          color: 'var(--color-text-secondary)',
          fontSize: 'var(--text-xs)',
          fontWeight: 500,
        }}
      >
        {label}
      </span>
      <span style={{ flex: 1, color: 'var(--color-text)' }}>{value}</span>
    </div>
  );
}

function ModelRow({
  icon,
  label,
  model,
}: {
  icon: string;
  label: string;
  model: string;
}): JSX.Element {
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-3)',
        padding: 'var(--space-2) var(--space-3)',
        background: 'var(--color-bg)',
        borderRadius: 'var(--radius-sm)',
      }}
    >
      <span aria-hidden="true" style={{ fontSize: 18 }}>
        {icon}
      </span>
      <span style={{ flex: 1, fontSize: 'var(--text-sm)', fontWeight: 500 }}>
        {label}
      </span>
      <code
        style={{
          fontFamily: 'var(--font-mono)',
          fontSize: 'var(--text-xs)',
          color: 'var(--color-accent)',
          background: 'var(--color-surface)',
          padding: '2px var(--space-2)',
          borderRadius: 'var(--radius-sm)',
        }}
      >
        {model}
      </code>
    </div>
  );
}
