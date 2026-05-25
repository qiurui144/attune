/** OfficeView · v0.7.1 — Office helper (OCR + ASR transcription).
 *
 * Spec: docs/superpowers/specs/2026-05-20-office-helper-design.md
 * Two panels:
 *   📷 OCR — file upload → profile picker → structured result + raw lines
 *   🎙️ Transcribe — file path → async job → WS progress → transcript display
 *
 * Per spec §1 architecture: results do NOT auto-save to vault (tool semantics).
 * User has option to copy/download but not persist.
 */

import type { JSX } from 'preact';
import { useSignal } from '@preact/signals';
import { useEffect } from 'preact/hooks';
import { Button, EmptyState } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import {
  runOcr,
  submitTranscribe,
  openJobWebSocket,
  cancelJob,
  type OcrResponse,
  type ProgressFrame,
  type JobState,
  type JobStage,
} from '../hooks/useOfficeJob';

type Tab = 'ocr' | 'transcribe';

export function OfficeView(): JSX.Element {
  const tab = useSignal<Tab>('ocr');

  return (
    <div style={{ padding: 'var(--space-6)', maxWidth: 1200, margin: '0 auto' }}>
      <header style={{ marginBottom: 'var(--space-5)' }}>
        <h1 style={{ fontSize: 'var(--text-2xl)', fontWeight: 600, margin: 0 }}>
          📋 {t('office.title')}
        </h1>
        <p style={{ color: 'var(--color-text-muted)', marginTop: 'var(--space-2)' }}>
          {t('office.subtitle')}
        </p>
      </header>

      <div
        role="tablist"
        aria-label={t('office.tabs.aria')}
        style={{
          display: 'flex',
          gap: 'var(--space-2)',
          marginBottom: 'var(--space-5)',
          borderBottom: '1px solid var(--color-border)',
        }}
      >
        <TabButton active={tab.value === 'ocr'} onClick={() => (tab.value = 'ocr')}>
          📷 {t('office.tab.ocr')}
        </TabButton>
        <TabButton active={tab.value === 'transcribe'} onClick={() => (tab.value = 'transcribe')}>
          🎙️ {t('office.tab.transcribe')}
        </TabButton>
      </div>

      {tab.value === 'ocr' && <OcrPanel />}
      {tab.value === 'transcribe' && <TranscribePanel />}
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: preact.ComponentChildren;
}): JSX.Element {
  return (
    <button
      role="tab"
      aria-selected={active}
      onClick={onClick}
      style={{
        padding: 'var(--space-3) var(--space-4)',
        border: 'none',
        background: 'transparent',
        color: active ? 'var(--color-text)' : 'var(--color-text-muted)',
        fontWeight: active ? 600 : 400,
        borderBottom: active ? '2px solid var(--color-accent)' : '2px solid transparent',
        marginBottom: -1,
        cursor: 'pointer',
        fontSize: 'var(--text-base)',
      }}
    >
      {children}
    </button>
  );
}

// ────────────────────────────────────────────────────────────────────
// OCR Panel
// ────────────────────────────────────────────────────────────────────

const OCR_PROFILES = [
  'document',
  'receipt',
  'table',
  'card',
  'id_card',
  'screenshot',
  'ancient',
  'form',
  'contract',
] as const;

const ID_CARD_SUBTYPES = ['id_card_cn', 'bank_card', 'business_license'] as const;

function OcrPanel(): JSX.Element {
  const file = useSignal<File | null>(null);
  const profile = useSignal<string>('receipt');
  const idCardSubtype = useSignal<string>('id_card_cn');
  const loading = useSignal(false);
  const result = useSignal<OcrResponse | null>(null);

  async function runExtract() {
    if (!file.value) {
      toast('error', t('office.ocr.error.no_file'));
      return;
    }
    if (profile.value === 'id_card' && !idCardSubtype.value) {
      toast('error', t('office.ocr.error.no_subtype'));
      return;
    }

    loading.value = true;
    result.value = null;
    try {
      const r = await runOcr(
        file.value,
        profile.value,
        profile.value === 'id_card' ? idCardSubtype.value : undefined,
      );
      result.value = r;
      toast('success', t('office.ocr.toast.success', { ms: String(r.elapsed_ms) }));
    } catch (e) {
      const err = e as Error & { code?: string };
      toast('error', t('office.ocr.toast.failed', { msg: err.message, code: err.code ?? '' }));
    } finally {
      loading.value = false;
    }
  }

  function onFileSelect(ev: Event) {
    const target = ev.target as HTMLInputElement;
    if (target.files && target.files[0]) {
      file.value = target.files[0];
    }
  }

  return (
    <div style={{ display: 'flex', gap: 'var(--space-5)' }}>
      {/* Left: form */}
      <section
        style={{
          flex: '0 0 320px',
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-3)',
        }}
      >
        <label
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--space-2)',
          }}
        >
          <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-muted)' }}>
            {t('office.ocr.label.file')}
          </span>
          <input
            type="file"
            accept=".pdf,.png,.jpg,.jpeg,.webp,.bmp,.tiff,.tif,.gif"
            onChange={onFileSelect}
            aria-label={t('office.ocr.label.file')}
          />
          {file.value && (
            <span style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)' }}>
              {file.value.name} · {(file.value.size / 1024).toFixed(1)} KB
            </span>
          )}
        </label>

        <label
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--space-2)',
          }}
        >
          <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-muted)' }}>
            {t('office.ocr.label.profile')}
          </span>
          <select
            value={profile.value}
            onChange={(e) => (profile.value = (e.target as HTMLSelectElement).value)}
            aria-label={t('office.ocr.label.profile')}
            style={{
              padding: 'var(--space-2)',
              border: '1px solid var(--color-border)',
              borderRadius: 'var(--radius-base)',
              background: 'var(--color-bg)',
              color: 'var(--color-text)',
            }}
          >
            {OCR_PROFILES.map((p) => (
              <option value={p}>{t(`office.profile.${p}`)}</option>
            ))}
          </select>
        </label>

        {profile.value === 'id_card' && (
          <label
            style={{
              display: 'flex',
              flexDirection: 'column',
              gap: 'var(--space-2)',
            }}
          >
            <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-muted)' }}>
              {t('office.ocr.label.id_card_subtype')}
            </span>
            <select
              value={idCardSubtype.value}
              onChange={(e) => (idCardSubtype.value = (e.target as HTMLSelectElement).value)}
              aria-label={t('office.ocr.label.id_card_subtype')}
              style={{
                padding: 'var(--space-2)',
                border: '1px solid var(--color-border)',
                borderRadius: 'var(--radius-base)',
                background: 'var(--color-bg)',
                color: 'var(--color-text)',
              }}
            >
              {ID_CARD_SUBTYPES.map((s) => (
                <option value={s}>{t(`office.id_card_subtype.${s}`)}</option>
              ))}
            </select>
          </label>
        )}

        <Button
          variant="primary"
          onClick={runExtract}
          disabled={!file.value || loading.value}
        >
          {loading.value ? t('office.ocr.button.running') : t('office.ocr.button.run')}
        </Button>
      </section>

      {/* Right: results */}
      <section style={{ flex: 1, minWidth: 0 }}>
        {!result.value && (
          <EmptyState
            icon="📷"
            title={t('office.ocr.empty.title')}
            description={t('office.ocr.empty.description')}
          />
        )}
        {result.value && <OcrResultDisplay result={result.value} />}
      </section>
    </div>
  );
}

function OcrResultDisplay({ result }: { result: OcrResponse }): JSX.Element {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-4)' }}>
      <div
        style={{
          padding: 'var(--space-3)',
          background: 'var(--color-surface-elevated)',
          borderRadius: 'var(--radius-base)',
          fontSize: 'var(--text-sm)',
          color: 'var(--color-text-muted)',
        }}
      >
        <strong>{t('office.ocr.result.engine')}</strong>: {result.engine} ·{' '}
        <strong>{t('office.ocr.result.elapsed')}</strong>: {result.elapsed_ms} ms ·{' '}
        <strong>{t('office.ocr.result.profile')}</strong>: {result.profile} ·{' '}
        <strong>{t('office.ocr.result.lines')}</strong>: {result.lines.length}
      </div>

      {result.warnings && result.warnings.length > 0 && (
        <div
          style={{
            padding: 'var(--space-3)',
            background: 'var(--color-warning-bg, #fef3c7)',
            borderRadius: 'var(--radius-base)',
            fontSize: 'var(--text-sm)',
          }}
        >
          <strong>{t('office.ocr.result.warnings')}</strong>
          <ul style={{ margin: 'var(--space-1) 0 0', paddingLeft: 'var(--space-5)' }}>
            {result.warnings.map((w) => (
              <li>{w}</li>
            ))}
          </ul>
        </div>
      )}

      {result.structured && <StructuredFieldsBlock structured={result.structured} />}

      <details>
        <summary
          style={{
            cursor: 'pointer',
            padding: 'var(--space-2)',
            color: 'var(--color-text-muted)',
            fontSize: 'var(--text-sm)',
          }}
        >
          {t('office.ocr.result.raw_lines_toggle', { count: String(result.lines.length) })}
        </summary>
        <pre
          style={{
            maxHeight: 400,
            overflow: 'auto',
            padding: 'var(--space-3)',
            background: 'var(--color-surface-elevated)',
            borderRadius: 'var(--radius-base)',
            fontSize: 'var(--text-xs)',
            fontFamily: 'monospace',
          }}
        >
          {result.lines.map((l) => l.text).join('\n')}
        </pre>
      </details>
    </div>
  );
}

function StructuredFieldsBlock({
  structured,
}: {
  structured: OcrResponse['structured'];
}): JSX.Element {
  if (!structured) return <></>;

  return (
    <div>
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          marginBottom: 'var(--space-2)',
        }}
      >
        <h3 style={{ margin: 0, fontSize: 'var(--text-lg)' }}>
          {t('office.ocr.result.structured')} · <code>{structured.schema}</code>
        </h3>
      </div>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'auto 1fr',
          gap: 'var(--space-2) var(--space-4)',
          padding: 'var(--space-3)',
          background: 'var(--color-surface-elevated)',
          borderRadius: 'var(--radius-base)',
        }}
      >
        {Object.entries(structured.fields).map(([key, val]) => (
          <FieldRow key={key} fieldKey={key} value={val} />
        ))}
      </div>
      {structured.unrecognized_fields && structured.unrecognized_fields.length > 0 && (
        <p style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-muted)', marginTop: 'var(--space-2)' }}>
          {t('office.ocr.result.unrecognized', { fields: structured.unrecognized_fields.join(', ') })}
        </p>
      )}
      {structured.validation_warnings && structured.validation_warnings.length > 0 && (
        <ul style={{ fontSize: 'var(--text-xs)', color: 'var(--color-warning, #b45309)', marginTop: 'var(--space-2)' }}>
          {structured.validation_warnings.map((w) => (
            <li>⚠ {w}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

function FieldRow({ fieldKey, value }: { fieldKey: string; value: unknown }): JSX.Element {
  // value can be FieldValue {value, confidence, bbox?, source_line_idx?}
  // or for document_v1 blocks, an array. Render gracefully.
  const isFieldValue =
    typeof value === 'object' &&
    value !== null &&
    'value' in (value as Record<string, unknown>) &&
    'confidence' in (value as Record<string, unknown>);

  if (isFieldValue) {
    const fv = value as { value: string | null; confidence: number };
    const display = fv.value ?? t('office.ocr.field.unrecognized');
    const lowConfidence = fv.confidence < 0.6;
    return (
      <>
        <span
          style={{
            fontWeight: 500,
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text-muted)',
          }}
        >
          {fieldKey}
        </span>
        <span
          style={{
            fontSize: 'var(--text-sm)',
            color: fv.value === null ? 'var(--color-text-muted)' : 'var(--color-text)',
            background: lowConfidence ? 'var(--color-warning-bg, #fef3c7)' : 'transparent',
            padding: lowConfidence ? '0 var(--space-1)' : 0,
            borderRadius: 'var(--radius-sm)',
            fontStyle: fv.value === null ? 'italic' : 'normal',
          }}
          title={`confidence ${fv.confidence.toFixed(2)}`}
        >
          {display}{' '}
          <small style={{ color: 'var(--color-text-muted)' }}>({fv.confidence.toFixed(2)})</small>
        </span>
      </>
    );
  }

  // Fallback for non-FieldValue shapes (e.g. document_v1.blocks array)
  return (
    <>
      <span style={{ fontWeight: 500, fontSize: 'var(--text-sm)' }}>{fieldKey}</span>
      <pre style={{ fontSize: 'var(--text-xs)', margin: 0 }}>
        {typeof value === 'string' ? value : JSON.stringify(value, null, 2)}
      </pre>
    </>
  );
}

// ────────────────────────────────────────────────────────────────────
// Transcribe Panel
// ────────────────────────────────────────────────────────────────────

function TranscribePanel(): JSX.Element {
  const filePath = useSignal<string>('');
  const diarization = useSignal(false);
  const language = useSignal<string>('auto');
  const submitting = useSignal(false);
  const jobId = useSignal<string | null>(null);
  const state = useSignal<JobState>('queued');
  const stage = useSignal<JobStage>('queued');
  const progress = useSignal(0);
  const queuePos = useSignal(0);
  const elapsedMs = useSignal(0);
  const result = useSignal<unknown | null>(null);
  const errorMsg = useSignal<string | null>(null);
  const closeWs = useSignal<(() => void) | null>(null);

  // Cleanup WS on unmount
  useEffect(() => {
    return () => {
      if (closeWs.value) {
        closeWs.value();
      }
    };
  }, []);

  async function startTranscribe() {
    if (!filePath.value.trim()) {
      toast('error', t('office.transcribe.error.no_file'));
      return;
    }

    submitting.value = true;
    result.value = null;
    errorMsg.value = null;
    try {
      const r = await submitTranscribe(filePath.value, {
        diarization: diarization.value,
        language: language.value,
      });
      jobId.value = r.job_id;
      state.value = 'queued';
      stage.value = 'queued';
      progress.value = 0;

      // Open WS for live progress
      const close = openJobWebSocket(
        r.job_id,
        (frame: ProgressFrame) => {
          if (frame.type !== 'progress') return;
          state.value = frame.state;
          stage.value = frame.stage;
          progress.value = frame.progress;
          queuePos.value = frame.queue_position;
          elapsedMs.value = frame.elapsed_ms;
        },
        (res: unknown) => {
          state.value = 'done';
          result.value = res;
          toast('success', t('office.transcribe.toast.done'));
        },
        (err) => {
          state.value = 'failed';
          errorMsg.value = err instanceof Error ? err.message : err.message;
          toast('error', t('office.transcribe.toast.failed', { msg: errorMsg.value ?? '' }));
        },
      );
      closeWs.value = close;
    } catch (e) {
      const err = e as Error & { code?: string };
      errorMsg.value = err.message;
      toast('error', t('office.transcribe.toast.submit_failed', { msg: err.message }));
    } finally {
      submitting.value = false;
    }
  }

  async function handleCancel() {
    if (!jobId.value) return;
    const ok = await cancelJob(jobId.value);
    if (ok) {
      state.value = 'cancelled';
      toast('info', t('office.transcribe.toast.cancelled'));
    } else {
      toast('info', t('office.transcribe.toast.already_terminal'));
    }
    if (closeWs.value) {
      closeWs.value();
      closeWs.value = null;
    }
  }

  return (
    <div style={{ display: 'flex', gap: 'var(--space-5)' }}>
      {/* Left: form */}
      <section
        style={{
          flex: '0 0 360px',
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-3)',
        }}
      >
        <label
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--space-2)',
          }}
        >
          <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-muted)' }}>
            {t('office.transcribe.label.file_path')}
          </span>
          <input
            type="text"
            value={filePath.value}
            placeholder={t('office.transcribe.placeholder.file_path')}
            onInput={(e) => (filePath.value = (e.target as HTMLInputElement).value)}
            aria-label={t('office.transcribe.label.file_path')}
            style={{
              padding: 'var(--space-2)',
              border: '1px solid var(--color-border)',
              borderRadius: 'var(--radius-base)',
              background: 'var(--color-bg)',
              color: 'var(--color-text)',
              fontFamily: 'monospace',
              fontSize: 'var(--text-sm)',
            }}
          />
        </label>

        <label
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--space-2)',
          }}
        >
          <span style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text-muted)' }}>
            {t('office.transcribe.label.language')}
          </span>
          <select
            value={language.value}
            onChange={(e) => (language.value = (e.target as HTMLSelectElement).value)}
            aria-label={t('office.transcribe.label.language')}
            style={{
              padding: 'var(--space-2)',
              border: '1px solid var(--color-border)',
              borderRadius: 'var(--radius-base)',
              background: 'var(--color-bg)',
              color: 'var(--color-text)',
            }}
          >
            <option value="auto">{t('office.transcribe.lang.auto')}</option>
            <option value="zh">{t('office.transcribe.lang.zh')}</option>
            <option value="en">{t('office.transcribe.lang.en')}</option>
          </select>
        </label>

        <label
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 'var(--space-2)',
            fontSize: 'var(--text-sm)',
          }}
        >
          <input
            type="checkbox"
            checked={diarization.value}
            onChange={(e) => (diarization.value = (e.target as HTMLInputElement).checked)}
            aria-label={t('office.transcribe.label.diarization')}
          />
          <span>{t('office.transcribe.label.diarization')}</span>
        </label>

        <Button
          variant="primary"
          onClick={startTranscribe}
          disabled={!filePath.value.trim() || submitting.value}
        >
          {submitting.value
            ? t('office.transcribe.button.submitting')
            : t('office.transcribe.button.start')}
        </Button>

        {jobId.value && state.value !== 'done' && state.value !== 'failed' && state.value !== 'cancelled' && (
          <Button variant="secondary" onClick={handleCancel}>
            {t('office.transcribe.button.cancel')}
          </Button>
        )}
      </section>

      {/* Right: progress + result */}
      <section style={{ flex: 1, minWidth: 0 }}>
        {!jobId.value && (
          <EmptyState
            icon="🎙️"
            title={t('office.transcribe.empty.title')}
            description={t('office.transcribe.empty.description')}
          />
        )}

        {jobId.value && (
          <TranscribeStatusDisplay
            jobId={jobId.value}
            state={state.value}
            stage={stage.value}
            progress={progress.value}
            queuePos={queuePos.value}
            elapsedMs={elapsedMs.value}
            result={result.value}
            errorMsg={errorMsg.value}
          />
        )}
      </section>
    </div>
  );
}

function TranscribeStatusDisplay({
  jobId,
  state,
  stage,
  progress,
  queuePos,
  elapsedMs,
  result,
  errorMsg,
}: {
  jobId: string;
  state: JobState;
  stage: JobStage;
  progress: number;
  queuePos: number;
  elapsedMs: number;
  result: unknown | null;
  errorMsg: string | null;
}): JSX.Element {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-4)' }}>
      <div
        style={{
          padding: 'var(--space-3)',
          background: 'var(--color-surface-elevated)',
          borderRadius: 'var(--radius-base)',
          fontSize: 'var(--text-sm)',
        }}
      >
        <strong>{t('office.transcribe.status.job_id')}</strong>:{' '}
        <code style={{ fontFamily: 'monospace' }}>{jobId}</code>
        <br />
        <strong>{t('office.transcribe.status.state')}</strong>: {t(`office.transcribe.state.${state}`)} ·{' '}
        <strong>{t('office.transcribe.status.stage')}</strong>: {t(`office.transcribe.stage.${stage}`)}
        {state === 'queued' && queuePos > 0 && (
          <>
            {' · '}
            <span style={{ color: 'var(--color-text-muted)' }}>
              {t('office.transcribe.status.queue_position', { pos: String(queuePos) })}
            </span>
          </>
        )}
        {state === 'running' && (
          <>
            {' · '}
            <span>
              {(progress * 100).toFixed(0)}% · {(elapsedMs / 1000).toFixed(1)}s
            </span>
          </>
        )}
      </div>

      {state === 'running' && (
        <div
          style={{
            height: 8,
            background: 'var(--color-surface-elevated)',
            borderRadius: 'var(--radius-full)',
            overflow: 'hidden',
          }}
        >
          <div
            style={{
              width: `${(progress * 100).toFixed(0)}%`,
              height: '100%',
              background: 'var(--color-accent)',
              transition: 'width var(--duration-base)',
            }}
          />
        </div>
      )}

      {state === 'failed' && errorMsg && (
        <div
          style={{
            padding: 'var(--space-3)',
            background: 'var(--color-danger-bg, #fef2f2)',
            color: 'var(--color-danger, #b91c1c)',
            borderRadius: 'var(--radius-base)',
          }}
        >
          {t('office.transcribe.error.failed_prefix')} {errorMsg}
        </div>
      )}

      {state === 'done' && result !== null && <TranscriptResult result={result} />}
    </div>
  );
}

function TranscriptResult({ result }: { result: unknown }): JSX.Element {
  // Result shape per spec §3.2: {model, language_detected, duration_sec, segments[], speakers[], full_text, diarization_used}
  const r = result as {
    model?: string;
    language_detected?: string;
    duration_sec?: number;
    segments?: Array<{ start_sec: number; end_sec: number; text: string; speaker?: string | null }>;
    speakers?: Array<{ id: string; total_sec: number; segment_count: number }>;
    full_text?: string;
    diarization_used?: boolean;
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-4)' }}>
      <div
        style={{
          padding: 'var(--space-3)',
          background: 'var(--color-surface-elevated)',
          borderRadius: 'var(--radius-base)',
          fontSize: 'var(--text-sm)',
        }}
      >
        <strong>{t('office.transcribe.result.model')}</strong>: {r.model ?? '?'} ·{' '}
        <strong>{t('office.transcribe.result.lang')}</strong>: {r.language_detected ?? '?'} ·{' '}
        <strong>{t('office.transcribe.result.duration')}</strong>:{' '}
        {r.duration_sec !== undefined ? r.duration_sec.toFixed(1) : '?'} s ·{' '}
        <strong>{t('office.transcribe.result.diarization')}</strong>:{' '}
        {r.diarization_used ? t('common.yes') : t('common.no')}
      </div>

      {r.speakers && r.speakers.length > 0 && (
        <div>
          <h4 style={{ margin: '0 0 var(--space-2)', fontSize: 'var(--text-base)' }}>
            {t('office.transcribe.result.speakers')}
          </h4>
          <ul style={{ margin: 0, paddingLeft: 'var(--space-5)' }}>
            {r.speakers.map((sp) => (
              <li>
                <strong>{sp.id}</strong> · {sp.total_sec.toFixed(1)} s · {sp.segment_count}{' '}
                {t('office.transcribe.result.segments_count')}
              </li>
            ))}
          </ul>
        </div>
      )}

      <div>
        <h4 style={{ margin: '0 0 var(--space-2)', fontSize: 'var(--text-base)' }}>
          {t('office.transcribe.result.transcript')}
        </h4>
        <div
          style={{
            maxHeight: 400,
            overflow: 'auto',
            padding: 'var(--space-3)',
            background: 'var(--color-surface-elevated)',
            borderRadius: 'var(--radius-base)',
            fontSize: 'var(--text-sm)',
          }}
        >
          {r.segments && r.segments.length > 0 ? (
            r.segments.map((seg) => (
              <div style={{ marginBottom: 'var(--space-2)' }}>
                <span
                  style={{
                    color: 'var(--color-text-muted)',
                    fontFamily: 'monospace',
                    fontSize: 'var(--text-xs)',
                    marginRight: 'var(--space-2)',
                  }}
                >
                  [{seg.start_sec.toFixed(1)}–{seg.end_sec.toFixed(1)}s]
                </span>
                {seg.speaker && (
                  <span
                    style={{
                      color: 'var(--color-accent)',
                      fontWeight: 500,
                      marginRight: 'var(--space-2)',
                    }}
                  >
                    {seg.speaker}:
                  </span>
                )}
                <span>{seg.text}</span>
              </div>
            ))
          ) : (
            <em style={{ color: 'var(--color-text-muted)' }}>{r.full_text ?? ''}</em>
          )}
        </div>
      </div>
    </div>
  );
}
