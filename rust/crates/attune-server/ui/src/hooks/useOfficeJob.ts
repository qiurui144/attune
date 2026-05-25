/**
 * useOfficeJob — Office helper REST + WebSocket helper for OCR/ASR.
 *
 * Spec: docs/superpowers/specs/2026-05-20-office-helper-design.md §3
 *
 * Exports:
 *   - runOcr(file, profile, idCardSubtype?) → OcrResponse (sync REST)
 *   - submitTranscribe(filePath, opts) → { job_id, ws_url }
 *   - openJobWebSocket(jobId, onProgress, onDone, onError) → close fn
 *   - getJob(jobId) → JobStatus
 *   - cancelJob(jobId) → void
 */

import { api } from '../store/api';

// ─── Types matching spec §3 envelope ───────────────────────────────────

export type BBox = { x: number; y: number; w: number; h: number };

export type RawLine = {
  text: string;
  bbox: BBox;
  confidence: number;
};

export type FieldValue = {
  value: string | null;
  confidence: number;
  bbox?: BBox;
  source_line_idx?: number;
};

/** Tagged union per spec §4.4 (path Y). client uses .schema as discriminator. */
export type StructuredFields =
  | { schema: 'document_v1'; fields: Record<string, unknown>; unrecognized_fields?: string[]; validation_warnings?: string[] }
  | { schema: 'receipt_v1'; fields: Record<string, FieldValue>; unrecognized_fields?: string[]; validation_warnings?: string[] }
  | { schema: 'table_v1'; fields: Record<string, FieldValue>; unrecognized_fields?: string[]; validation_warnings?: string[] }
  | { schema: 'card_v1'; fields: Record<string, FieldValue>; unrecognized_fields?: string[]; validation_warnings?: string[] }
  | { schema: 'id_card_cn_v1'; fields: Record<string, FieldValue>; unrecognized_fields?: string[]; validation_warnings?: string[] }
  | { schema: 'bank_card_v1'; fields: Record<string, FieldValue>; unrecognized_fields?: string[]; validation_warnings?: string[] }
  | { schema: 'business_license_v1'; fields: Record<string, FieldValue>; unrecognized_fields?: string[]; validation_warnings?: string[] };

export type OcrResponse = {
  envelope_version: string;
  profile: string;
  elapsed_ms: number;
  engine: string;
  lines: RawLine[];
  structured: StructuredFields | null;
  warnings?: string[];
};

export type JobState = 'queued' | 'running' | 'done' | 'failed' | 'cancelled';
export type JobStage = 'queued' | 'loading_model' | 'transcribing' | 'diarizing' | 'postprocess';

export type JobStatus = {
  job_id: string;
  state: JobState;
  stage: JobStage;
  queue_position: number;
  progress: number;
  elapsed_ms: number;
  eta_ms: number | null;
  result: unknown | null;
  error: { message: string; code: string } | null;
  warnings: string[];
};

export type TranscribeOpts = {
  language?: string;          // "auto" | "zh" | "en"
  model?: string;             // "small" | "medium" | "large-v3-turbo"
  diarization?: boolean;
  max_speakers?: number;
};

// ─── OCR (sync REST) ──────────────────────────────────────────────────

/** Run OCR on a File via multipart POST /api/v1/office/ocr. */
export async function runOcr(
  file: File,
  profile: string,
  idCardSubtype?: string,
): Promise<OcrResponse> {
  const form = new FormData();
  form.append('file', file);
  form.append('profile', profile);
  if (idCardSubtype) {
    form.append('id_card_subtype', idCardSubtype);
  }

  // We can't use the JSON api helper here — multipart needs raw fetch.
  // Authentication via token header (matches api.ts logic).
  const token = localStorage.getItem('attune_token');
  const headers: Record<string, string> = {};
  if (token) headers['Authorization'] = `Bearer ${token}`;

  const resp = await fetch('/api/v1/office/ocr', {
    method: 'POST',
    headers,
    body: form,
  });

  if (!resp.ok) {
    const body = await resp.json().catch(() => ({ error: 'unknown', code: 'unknown' }));
    const err = new Error(body.error ?? `HTTP ${resp.status}`);
    (err as Error & { code?: string }).code = body.code;
    throw err;
  }

  return (await resp.json()) as OcrResponse;
}

// ─── ASR (async + WebSocket) ──────────────────────────────────────────

/** Submit ASR job. Returns job_id + ws_url. */
export async function submitTranscribe(
  filePath: string,
  opts: TranscribeOpts = {},
): Promise<{ job_id: string; ws_url: string }> {
  return await api.post('/office/transcribe', {
    file_path: filePath,
    language: opts.language ?? 'auto',
    model: opts.model ?? 'small',
    diarization: opts.diarization ?? false,
    max_speakers: opts.max_speakers,
  });
}

/** Poll a job's current status. */
export async function getJob(jobId: string): Promise<JobStatus> {
  return await api.get(`/office/jobs/${encodeURIComponent(jobId)}`);
}

/** Cancel a running/queued job. Returns success boolean (true on 204; false on 409 already-terminal). */
export async function cancelJob(jobId: string): Promise<boolean> {
  try {
    await api.delete(`/office/jobs/${encodeURIComponent(jobId)}`);
    return true;
  } catch (e) {
    // 409 = already done/failed/cancelled — surfaced as ApiError
    const code = (e as Error & { code?: string }).code;
    if (code === 'job-already-completed' || code === 'job-already-cancelled') {
      return false;
    }
    throw e;
  }
}

// ─── WS progress wrapper ──────────────────────────────────────────────

export type ProgressFrame =
  | { type: 'progress'; job_id: string; state: JobState; stage: JobStage; queue_position: number; progress: number; elapsed_ms: number }
  | { type: 'done'; job_id: string; result: unknown }
  | { type: 'failed'; job_id: string; error: { message: string; code: string } }
  | { type: 'cancelled'; job_id: string };

/**
 * Open a WebSocket to track job progress. Returns a close() function.
 *
 * onProgress fires on each progress tick; onDone on completion;
 * onError on failed/cancelled or transport error.
 */
export function openJobWebSocket(
  jobId: string,
  onProgress: (frame: ProgressFrame) => void,
  onDone: (result: unknown) => void,
  onError: (err: { message: string; code: string } | Error) => void,
): () => void {
  // Build ws URL relative to current page (works for http/https + ws/wss).
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const url = `${proto}//${window.location.host}/api/v1/office/jobs/ws?job_id=${encodeURIComponent(jobId)}`;
  const ws = new WebSocket(url);

  ws.onmessage = (ev: MessageEvent) => {
    let frame: ProgressFrame;
    try {
      frame = JSON.parse(String(ev.data));
    } catch {
      return;
    }
    if (frame.type === 'progress') {
      onProgress(frame);
    } else if (frame.type === 'done') {
      onDone(frame.result);
      ws.close();
    } else if (frame.type === 'failed') {
      onError(frame.error);
      ws.close();
    } else if (frame.type === 'cancelled') {
      onError({ message: 'cancelled', code: 'job-cancelled' });
      ws.close();
    }
  };

  ws.onerror = () => {
    onError(new Error('WebSocket transport error'));
  };

  return () => {
    if (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING) {
      // Send cancel sentinel before close (server reads {type:cancel} text frame).
      try {
        ws.send(JSON.stringify({ type: 'cancel', job_id: jobId }));
      } catch {
        /* ignore */
      }
      ws.close();
    }
  };
}
