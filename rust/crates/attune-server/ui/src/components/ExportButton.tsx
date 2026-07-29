/** ExportButton — render an {@link ExportArtifact} to a downloadable office file.
 *
 * 🆓 zero-cost (no LLM, no member gate). Serves the user need "输出表格并下载" /
 * "可直接下载的 doc 或 pdf". Shows a small format menu (xlsx/csv/md/docx/pdf); the
 * available formats can be narrowed by the caller (e.g. a table offers
 * xlsx/csv/md/pdf, a document offers md/docx/pdf).
 *
 * i18n (project §i18n): every visible string goes through t(); no hardcoded CJK.
 */

import { useState } from 'preact/hooks';
import type { JSX } from 'preact';
import { Button } from './Button';
import { toast } from './Toast';
import { t } from '../i18n';
import {
  exportArtifact,
  ExportError,
  type ExportArtifact,
  type ExportFormat,
} from '../hooks/useExport';

const ALL_FORMATS: ExportFormat[] = ['xlsx', 'csv', 'md', 'docx', 'pdf'];

export interface ExportButtonProps {
  /** The artifact to render (or a lazy producer, evaluated on click). */
  artifact: ExportArtifact | (() => ExportArtifact);
  /** Restrict the offered formats; defaults to all five. */
  formats?: ExportFormat[];
  /** Suggested download filename (no extension). */
  filename?: string;
  /** Button size passthrough. */
  size?: 'sm' | 'md' | 'lg';
  'data-testid'?: string;
}

const FORMAT_LABEL: Record<ExportFormat, string> = {
  xlsx: 'Excel (.xlsx)',
  csv: 'CSV (.csv)',
  md: 'Markdown (.md)',
  docx: 'Word (.docx)',
  pdf: 'PDF (.pdf)',
};

export function ExportButton(props: ExportButtonProps): JSX.Element {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<ExportFormat | null>(null);
  const formats = props.formats ?? ALL_FORMATS;

  async function run(format: ExportFormat): Promise<void> {
    setBusy(format);
    try {
      const artifact =
        typeof props.artifact === 'function' ? props.artifact() : props.artifact;
      await exportArtifact(artifact, format, props.filename);
      toast('success', t('export.success'));
      setOpen(false);
    } catch (e) {
      const msg =
        e instanceof ExportError && e.code === 'membership-required'
          ? t('export.error.membership')
          : t('export.error.generic');
      toast('error', msg);
    } finally {
      setBusy(null);
    }
  }

  return (
    <div style={{ position: 'relative', display: 'inline-block' }}>
      <Button
        variant="secondary"
        size={props.size ?? 'sm'}
        onClick={() => setOpen((v) => !v)}
        aria-label={t('export.button')}
        aria-pressed={open}
        data-testid={props['data-testid'] ?? 'export-button'}
      >
        {t('export.button')}
      </Button>
      {open && (
        <div
          role="menu"
          style={{
            position: 'absolute',
            top: '100%',
            right: 0,
            marginTop: '4px',
            background: 'var(--color-surface)',
            border: '1px solid var(--color-border)',
            borderRadius: '6px',
            boxShadow: '0 4px 12px rgba(36, 43, 55, 0.12)',
            zIndex: 20,
            minWidth: '160px',
            padding: '4px',
          }}
        >
          {formats.map((f) => (
            <button
              key={f}
              type="button"
              role="menuitem"
              disabled={busy !== null}
              onClick={() => void run(f)}
              data-testid={`export-format-${f}`}
              style={{
                display: 'block',
                width: '100%',
                textAlign: 'left',
                padding: '6px 10px',
                background: 'transparent',
                border: 'none',
                cursor: busy ? 'wait' : 'pointer',
                color: 'var(--color-text)',
                borderRadius: '4px',
              }}
            >
              {busy === f ? t('export.rendering') : FORMAT_LABEL[f]}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
