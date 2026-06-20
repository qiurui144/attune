/**
 * Artifact export client — POST /api/v1/export and trigger a browser download.
 *
 * Export is 🆓 zero-cost (no LLM, no member gate); the user need is literally
 * "输出表格并下载" / "可直接下载的 doc 或 pdf". The endpoint streams the rendered
 * file with a Content-Disposition filename; we honour it (with a sanitised
 * fallback) and click a transient anchor to save it.
 */

import { getToken } from '../store/api';

export type ExportFormat = 'xlsx' | 'csv' | 'md' | 'docx' | 'pdf';

/** Stable export IR — mirrors attune_core::export (adjacently tagged). */
export type ExportAlign = 'left' | 'center' | 'right';

export interface ExportTable {
  title?: string;
  headers: string[];
  rows: string[][];
  aligns?: ExportAlign[];
}

export type ExportBlock =
  | { kind: 'heading'; level: number; text: string }
  | { kind: 'paragraph'; text: string }
  | { kind: 'list'; ordered?: boolean; items: string[] }
  | ({ kind: 'table' } & ExportTable);

export interface ExportDocument {
  title?: string;
  blocks: ExportBlock[];
}

export type ExportArtifact =
  | { type: 'table'; data: ExportTable }
  | { type: 'document'; data: ExportDocument };

/** Build a Table artifact from headers + rows. */
export function tableArtifact(
  headers: string[],
  rows: string[][],
  opts?: { title?: string; aligns?: ExportAlign[] },
): ExportArtifact {
  return {
    type: 'table',
    data: { headers, rows, title: opts?.title, aligns: opts?.aligns },
  };
}

/** Build a Document artifact from ordered blocks. */
export function documentArtifact(
  blocks: ExportBlock[],
  title?: string,
): ExportArtifact {
  return { type: 'document', data: { title, blocks } };
}

/** Read the filename from a Content-Disposition header (prefers RFC-5987). */
function filenameFromDisposition(
  header: string | null,
  fallback: string,
): string {
  if (!header) return fallback;
  // filename*=UTF-8''<percent-encoded>
  const star = /filename\*=UTF-8''([^;]+)/i.exec(header);
  if (star?.[1]) {
    try {
      return decodeURIComponent(star[1]);
    } catch {
      /* fall through */
    }
  }
  const plain = /filename="?([^"]+)"?/i.exec(header);
  return plain?.[1] ?? fallback;
}

export class ExportError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
    this.name = 'ExportError';
  }
}

/**
 * Render `artifact` to `format` server-side and trigger a download.
 * Throws {@link ExportError} on a non-2xx (carrying the stable kebab `code`).
 */
export async function exportArtifact(
  artifact: ExportArtifact,
  format: ExportFormat,
  filename?: string,
): Promise<void> {
  const token = getToken();
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
  };
  if (token) headers.Authorization = `Bearer ${token}`;

  const res = await fetch('/api/v1/export', {
    method: 'POST',
    headers,
    body: JSON.stringify({ artifact, format, filename }),
  });

  if (!res.ok) {
    let code = 'export-failed';
    let message = `export failed (HTTP ${res.status})`;
    try {
      const body = (await res.json()) as { code?: string; error?: string };
      if (body.code) code = body.code;
      if (body.error) message = body.error;
    } catch {
      /* non-JSON error body */
    }
    throw new ExportError(res.status, code, message);
  }

  const blob = await res.blob();
  const name = filenameFromDisposition(
    res.headers.get('Content-Disposition'),
    `${filename ?? 'export'}.${format}`,
  );

  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  // revoke after the click has a chance to start the download
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
