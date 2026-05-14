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
  recommended_chat?: string | null;
  recommended_embedding?: string;
  recommended_summary?: string | null;
};

type DiagnosticsPayload = {
  hardware?: {
    os?: string;
    cpu_model?: string;
    total_ram_gb?: number;
    has_nvidia_gpu?: boolean;
    has_amd_gpu?: boolean;
    has_intel_igpu?: boolean;
    gpu_label?: string | null;
    has_amd_xdna_npu?: boolean;
    has_intel_npu?: boolean;
    recommended_summary_model?: string;
  };
};

type AiStackTier = {
  tier: 'unsupported' | 'low' | 'mid' | 'high' | 'flagship';
  supported: boolean;
  cpu_passmark?: number | null;
  npu_tops?: number | null;
};

type AiStackRecommendation = {
  embedding_repo: string;
  embedding_size_mb: number;
  reranker_repo: string;
  reranker_size_mb: number;
  asr_ggml: string;
  asr_size_mb: number;
  total_download_mb: number;
};

type AiStackResponse = {
  hardware: AiStackTier & { ram_gb?: number; has_gpu?: boolean };
  region: { detected: string; hf_endpoint: string };
  recommendation: AiStackRecommendation | null;
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
  ctx,
  onUpdate,
  onContinue,
}: Step4Props): JSX.Element {
  const [hw, setHw] = useState<HardwareInfo | null>(null);
  const [aiStack, setAiStack] = useState<AiStackResponse | null>(null);
  const [scanSteps, setScanSteps] = useState<ScanStep[]>([]);
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
        const [diag, stack] = await Promise.all([
          api.get<DiagnosticsPayload>('/status/diagnostics'),
          api.get<AiStackResponse>('/ai_stack'),
        ]);
        if (cancelled) return;
        setAiStack(stack);

        // 每 400ms tick 一阶段，视觉"扫描感"
        for (let i = 0; i < steps.length; i++) {
          await new Promise((r) => setTimeout(r, 400));
          if (cancelled) return;
          steps[i] = { ...steps[i]!, done: true };
          setScanSteps([...steps]);
        }

        const h = diag.hardware ?? {};
        const detectedGpu = h.has_nvidia_gpu
          ? 'NVIDIA GPU'
          : h.has_amd_gpu
            ? 'AMD GPU'
            : h.has_intel_igpu
              ? 'Intel iGPU'
            : null;
        const detectedNpu = h.has_amd_xdna_npu
          ? 'AMD XDNA'
          : h.has_intel_npu
            ? 'Intel NPU'
            : null;

        // 结果填充
        setHw({
          os: h.os,
          cpu_model: h.cpu_model ?? 'Unknown',
          gpu_model: h.gpu_label ?? detectedGpu,
          npu_type: detectedNpu,
          total_ram_gb: h.total_ram_gb,
          recommended_chat: null,
          recommended_embedding: 'bge-m3',
          recommended_summary: normalizeSummaryModel(h.recommended_summary_model),
        });
      } catch {
        // 失败时 fallback
        setHw({
          cpu_model: 'Unknown',
          total_ram_gb: 0,
          recommended_chat: null,
          recommended_embedding: 'bge-m3',
          recommended_summary: null,
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
      chatModel: null,
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

    onContinue();
  }

  // v0.6.0-rc.4: Tier 0 (unsupported) 拒绝继续，显示明确错误信息
  const tierUnsupported = aiStack?.hardware?.supported === false;
  const localChatBlocked =
    ctx.llmMode === 'ollama'
    && aiStack != null
    && (aiStack.hardware.tier === 'unsupported' || aiStack.hardware.tier === 'low' || aiStack.hardware.tier === 'mid');

  if (tierUnsupported && aiStack) {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-5)' }}>
        <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0, color: 'var(--color-danger)' }}>
          ⚠️ 设备规格不支持运行 Attune
        </h2>
        <div
          style={{
            background: 'var(--color-bg)',
            border: '1px solid var(--color-danger)',
            borderRadius: 'var(--radius-md)',
            padding: 'var(--space-4)',
            fontSize: 'var(--text-sm)',
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--space-2)',
          }}
        >
          <div>
            <strong>检测结果：</strong>
          </div>
          <div>· CPU: <code>{hw?.cpu_model ?? '-'}</code></div>
          {aiStack.hardware.cpu_passmark != null && (
            <div>· Passmark: <code>{aiStack.hardware.cpu_passmark}</code> (要求 ≥ 4000)</div>
          )}
          <div>· RAM: <code>{aiStack.hardware.ram_gb ?? '-'} GB</code> (要求 ≥ 4 GB)</div>
        </div>
        <div
          style={{
            background: 'var(--color-bg)',
            borderRadius: 'var(--radius-md)',
            padding: 'var(--space-4)',
            fontSize: 'var(--text-sm)',
          }}
        >
          <strong>推荐方案：</strong>
          <ul style={{ marginTop: 'var(--space-2)', paddingLeft: 'var(--space-4)' }}>
            <li>使用 K3 一体机（开箱即用，配本地 AI 全套）</li>
            <li>更换设备：8 核近代 CPU (Passmark ≥ 9000) + 8GB RAM</li>
          </ul>
        </div>
        <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
          <Button onClick={() => window.close?.()} variant="ghost">
            退出
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-5)' }}>
      <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0 }}>
        {t('wizard.hw.heading')}
      </h2>

      {aiStack && (
        <div
          style={{
            background: 'linear-gradient(180deg, var(--color-surface) 0%, var(--color-bg) 100%)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-md)',
            padding: 'var(--space-4)',
            fontSize: 'var(--text-sm)',
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--space-2)',
          }}
        >
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 'var(--space-2)' }}>
            <Pill label="硬件档位" value={aiStack.hardware.tier} />
            <Pill label="区域" value={aiStack.region.detected.split(' (')[0]} />
            <Pill label="底座" value="自动配置" />
          </div>
          <div style={{ color: 'var(--color-text-secondary)' }}>
            Embedding / Reranker / ASR / OCR 在后台完成，不占用你的注意力。
          </div>
        </div>
      )}

      <div
        style={{
          background: 'var(--color-surface)',
          borderRadius: 'var(--radius-md)',
          padding: 'var(--space-4)',
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-3)',
          border: '1px solid var(--color-border)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 'var(--space-3)' }}>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-1)' }}>
            <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>扫描进行中</div>
            <div style={{ fontSize: 'var(--text-sm)', fontWeight: 600 }}>检测硬件并匹配底座策略</div>
          </div>
          <div style={{ fontFamily: 'var(--font-mono)', fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
            {scanSteps.filter((s) => s.done).length}/{scanSteps.length}
          </div>
        </div>
        <div
          style={{
            height: 8,
            borderRadius: 999,
            background: 'var(--color-bg)',
            overflow: 'hidden',
          }}
        >
          <div
            style={{
              height: '100%',
              width: `${scanSteps.length ? (scanSteps.filter((s) => s.done).length / scanSteps.length) * 100 : 0}%`,
              background: 'linear-gradient(90deg, var(--color-accent) 0%, var(--color-success) 100%)',
              transition: 'width var(--duration-base) var(--ease-out)',
            }}
          />
        </div>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(140px, 1fr))', gap: 'var(--space-2)' }}>
          {scanSteps.map((s, i) => (
            <div
              key={i}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 'var(--space-2)',
                padding: 'var(--space-2) var(--space-3)',
                borderRadius: 'var(--radius-sm)',
                border: `1px solid ${s.done ? 'var(--color-success)' : 'var(--color-border)'}`,
                background: s.done ? 'rgba(34, 197, 94, 0.08)' : 'var(--color-bg)',
                color: s.done ? 'var(--color-success)' : 'var(--color-text-secondary)',
                fontSize: 'var(--text-xs)',
              }}
            >
              <span style={{ fontSize: 12 }}>{s.done ? '✓' : '·'}</span>
              <span>{s.label.replace('检测', '')}</span>
            </div>
          ))}
        </div>
      </div>

      {hw && (
        <div
          className="fade-in"
          style={{
            padding: 'var(--space-4)',
            background: 'linear-gradient(180deg, var(--color-surface) 0%, var(--color-bg) 100%)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-md)',
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--space-2)',
            fontSize: 'var(--text-sm)',
          }}
        >
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))', gap: 'var(--space-2)' }}>
            <MiniStat label="CPU" value={hw.cpu_model ?? '—'} />
            <MiniStat label="GPU" value={hw.gpu_model ?? '纯 CPU 模式'} />
            <MiniStat label="NPU" value={hw.npu_type ?? '—'} />
            <MiniStat label="RAM" value={`${hw.total_ram_gb ?? 0} GB`} />
          </div>
        </div>
      )}

      {localChatBlocked && (
        <div
          style={{
            padding: 'var(--space-3)',
            border: '1px solid var(--color-warning)',
            background: 'rgba(245, 158, 11, 0.08)',
            borderRadius: 'var(--radius-md)',
            color: 'var(--color-text-secondary)',
            fontSize: 'var(--text-sm)',
          }}
        >
          当前硬件规格不建议本地 Chat，流程已切换为云端 / K3 优先。
        </div>
      )}

      {hw && !localChatBlocked && (
        <div className="fade-slide-in" style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
          <div style={{ fontSize: 'var(--text-sm)', fontWeight: 600 }}>自动配置结果</div>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 'var(--space-2)' }}>
            <Pill label="对话模型" value="下一步选择" />
            <Pill label="向量索引" value="已自动配置" />
            <Pill label="本地摘要" value={displaySummaryLabel(hw.recommended_summary ?? null)} />
          </div>
        </div>
      )}

      <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
        <Button
          variant="primary"
          size="lg"
          loading={applying}
          disabled={!hw || localChatBlocked}
          onClick={applyRecommendation}
        >
          {t('wizard.hw.apply')} →
        </Button>
      </div>
    </div>
  );
}

function normalizeSummaryModel(model?: string | null): string | null {
  const v = model?.trim();
  if (!v) return null;
  // 大模型只做候选，不在 wizard 里直接展示为默认结果。
  if (/(:7b|:8b|:14b|:32b|35b|70b)/i.test(v)) {
    return null;
  }
  return v;
}

function displaySummaryLabel(model: string | null): string {
  return model ?? '自动（按硬件）';
}

function Pill({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <div
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 'var(--space-2)',
        padding: 'var(--space-2) var(--space-3)',
        borderRadius: '999px',
        background: 'var(--color-bg)',
        border: '1px solid var(--color-border)',
        fontSize: 'var(--text-xs)',
      }}
    >
      <span style={{ color: 'var(--color-text-secondary)', fontWeight: 600 }}>{label}</span>
      <span style={{ color: 'var(--color-text)' }}>{value}</span>
    </div>
  );
}

function MiniStat({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <div
      style={{
        padding: 'var(--space-3)',
        borderRadius: 'var(--radius-sm)',
        background: 'var(--color-bg)',
        border: '1px solid var(--color-border)',
      }}
    >
      <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', marginBottom: 'var(--space-1)' }}>
        {label}
      </div>
      <div style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text)' }}>{value}</div>
    </div>
  );
}

