/**
 * useFilePicker — shared file/directory picker abstraction over Tauri desktop
 * native dialog and browser hidden <input> fallback.
 *
 * WHY a hook: 3 existing callers (wizard Step5, Settings FolderLinks, RemoteView
 * LocalForm) all copy-pasted the same canPickFolder detection + @tauri-apps/plugin-dialog
 * dynamic import. This eliminates that duplication and adds 6 new consumption points
 * with a consistent API.
 */
import { signal } from '@preact/signals';

export interface PickFilesOptions {
  multiple?: boolean;
  accept?: string;   // HTML accept string ".pdf,.jpg"
  title?: string;
}

export interface PickFilesResult {
  paths: string[];
  files: File[];
}

export interface PickDirectoryOptions {
  multiple?: boolean;
  title?: string;
}

interface LocalFilePayload {
  file_name: string;
  bytes: number[];
}

function isTauriRuntime(): boolean {
  return typeof window !== 'undefined'
    && Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);
}

function basename(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/').filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : 'selected-file';
}

function mimeFromName(name: string): string {
  const ext = name.toLowerCase().split('.').pop() ?? '';
  const map: Record<string, string> = {
    pdf: 'application/pdf',
    txt: 'text/plain',
    md: 'text/markdown',
    csv: 'text/csv',
    json: 'application/json',
    docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
    xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    png: 'image/png',
    jpg: 'image/jpeg',
    jpeg: 'image/jpeg',
    gif: 'image/gif',
    webp: 'image/webp',
    bmp: 'image/bmp',
    tif: 'image/tiff',
    tiff: 'image/tiff',
    wav: 'audio/wav',
    mp3: 'audio/mpeg',
    m4a: 'audio/mp4',
    ogg: 'audio/ogg',
    flac: 'audio/flac',
    aac: 'audio/aac',
    opus: 'audio/ogg',
  };
  return map[ext] ?? 'application/octet-stream';
}

function fileUrlFromPath(path: string): string {
  const normalized = path.replace(/\\/g, '/');
  if (/^[A-Za-z]:\//.test(normalized)) return `file:///${encodeURI(normalized)}`;
  if (normalized.startsWith('/')) return `file://${encodeURI(normalized)}`;
  return `file://${encodeURI(normalized)}`;
}

async function readDesktopPathAsFile(path: string): Promise<File | null> {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    const payload = await invoke<LocalFilePayload>('read_local_file', { path });
    const name = payload.file_name || basename(path);
    const type = mimeFromName(name);
    const blob = new Blob([new Uint8Array(payload.bytes)], { type });
    return new File([blob], name, { type });
  } catch {
    // In tests and some WebView configurations the command may be unavailable;
    // fall through to the WebView file:// fetch path.
  }

  try {
    const name = basename(path);
    const resp = await fetch(fileUrlFromPath(path));
    if (!resp.ok) return null;
    const blob = await resp.blob();
    const type = blob.type || mimeFromName(name);
    return new File([blob], name, { type });
  } catch {
    return null;
  }
}

async function materializeDesktopFiles(paths: string[]): Promise<File[]> {
  const files: File[] = [];
  for (const path of paths) {
    const file = await readDesktopPathAsFile(path);
    if (file) files.push(file);
  }
  return files;
}

/**
 * Convert an HTML accept string (".pdf,.jpg,audio/*") to Tauri dialog filters.
 * Tauri filters have a human-readable name and an extensions list.
 */
function acceptToTauriFilters(accept: string): { name: string; extensions: string[] } {
  const parts = accept.split(',').map((s) => s.trim()).filter(Boolean);
  const extSet = new Set<string>();
  for (const part of parts) {
    if (part.startsWith('.')) {
      extSet.add(part.slice(1));
    } else if (part.includes('/*')) {
      const group = part.split('/')[0];
      const groupExts: string[] = [];
      if (group === 'audio') {
        groupExts.push('wav', 'mp3', 'm4a', 'ogg', 'flac', 'aac', 'opus');
      } else if (group === 'video') {
        groupExts.push('mp4', 'webm', 'mkv', 'avi', 'mov');
      } else if (group === 'image') {
        groupExts.push('png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg');
      }
      for (const ext of groupExts) extSet.add(ext);
    }
    // MIME types like "application/octet-stream" — skip for Tauri filters
  }
  const extensions = Array.from(extSet).sort();
  const label = parts
    .map((p) => {
      if (p.startsWith('.')) return p.slice(1).toUpperCase();
      // "audio/*" → "AUDIO", "application/octet-stream" → "APPLICATION"
      return p.split('/')[0].toUpperCase();
    })
    .join(', ')
    || 'All';
  return { name: label, extensions };
}

/**
 * Create a hidden <input type="file"> element for browser-based file selection.
 * Returns a promise that resolves with the File[] from the user's selection.
 */
function browserPickFiles(opts: {
  multiple: boolean;
  accept: string;
  directory: boolean;
}): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    input.style.display = 'none';
    input.multiple = opts.multiple;
    if (opts.accept) input.accept = opts.accept;
    if (opts.directory) {
      // @ts-ignore webkitdirectory is non-standard but widely supported in Chromium/Firefox
      input.webkitdirectory = true;
    }

    const cleanup = (): void => {
      if (input.parentNode) input.parentNode.removeChild(input);
    };

    input.addEventListener('change', () => {
      const files = input.files ? Array.from(input.files) : [];
      cleanup();
      resolve(files);
    }, { once: true });

    const fallbackTimer = setTimeout(() => {
      cleanup();
      resolve([]);
    }, 60_000);

    input.addEventListener('change', () => clearTimeout(fallbackTimer), { once: true });

    document.body.appendChild(input);
    input.click();
  });
}

export function useFilePicker() {
  const picking = signal(false);
  const error = signal<string | null>(null);
  const isDesktop = isTauriRuntime();

  async function pickDirectory(opts?: PickDirectoryOptions): Promise<string[]> {
    picking.value = true;
    error.value = null;
    try {
      if (isDesktop) {
        const { open } = await import('@tauri-apps/plugin-dialog');
        const selected = await open({
          directory: true,
          multiple: opts?.multiple !== false,
          title: opts?.title,
        });
        if (selected === null) return [];
        return Array.isArray(selected) ? selected : [selected];
      }

      // Browser fallback: webkitdirectory
      const files = await browserPickFiles({
        multiple: opts?.multiple !== false,
        accept: '',
        directory: true,
      });
      if (files.length === 0) return [];

      const dirSet = new Set<string>();
      for (const file of files) {
        const relPath = (file as any).webkitRelativePath as string | undefined;
        if (relPath) {
          const firstSlash = relPath.indexOf('/');
          dirSet.add(firstSlash >= 0 ? relPath.slice(0, firstSlash) : relPath);
        } else if ((file as any).path) {
          const fullPath = (file as any).path as string;
          const lastSlash = fullPath.lastIndexOf('/');
          if (lastSlash >= 0) dirSet.add(fullPath.slice(0, lastSlash));
        }
      }
      return Array.from(dirSet);
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
      return [];
    } finally {
      picking.value = false;
    }
  }

  async function pickFiles(opts?: PickFilesOptions): Promise<PickFilesResult> {
    picking.value = true;
    error.value = null;
    try {
      if (isDesktop) {
        const { open } = await import('@tauri-apps/plugin-dialog');
        const filters = opts?.accept
          ? [acceptToTauriFilters(opts.accept)]
          : undefined;
        const selected = await open({
          directory: false,
          multiple: opts?.multiple !== false,
          title: opts?.title,
          filters,
        });
        if (selected === null) return { paths: [], files: [] };
        const paths = Array.isArray(selected) ? selected : [selected];
        const files = await materializeDesktopFiles(paths);
        if (paths.length > 0 && files.length < paths.length) {
          error.value = 'Unable to read one or more selected files';
        }
        return { paths, files };
      }

      // Browser fallback: hidden file input
      const files = await browserPickFiles({
        multiple: opts?.multiple !== false,
        accept: opts?.accept ?? '',
        directory: false,
      });
      return { paths: [], files };
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
      return { paths: [], files: [] };
    } finally {
      picking.value = false;
    }
  }

  return {
    isDesktop,
    picking,
    error,
    pickDirectory,
    pickFiles,
  };
}
