/** useRemote · 已绑定目录（本地 + WebDAV）管理 */
import { api } from '../store/api';
import { ApiError } from '../store/api';

export type BoundDir = {
  id: string;
  path: string;
  recursive: boolean;
  file_types: string;
  last_scan?: string;
  kind: 'local' | 'webdav';
};

export type RemoteActionResult = {
  ok: boolean;
  error?: string;
};

type ListResponse = { directories: BoundDir[] };

export async function listBoundDirs(): Promise<BoundDir[]> {
  try {
    const res = await api.get<ListResponse>('/index/status');
    return res.directories ?? [];
  } catch {
    return [];
  }
}

export async function bindLocalDir(path: string): Promise<RemoteActionResult> {
  try {
    await api.post('/index/bind', { path, recursive: true });
    return { ok: true };
  } catch (e: unknown) {
    if (e instanceof ApiError) {
      return { ok: false, error: extractErrorMessage(e.body) };
    }
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export type WebdavInput = {
  url: string;
  username: string;
  password: string;
  remote_path: string;
};

export async function bindWebdav(input: WebdavInput): Promise<RemoteActionResult> {
  try {
    await api.post('/index/bind-remote', input);
    return { ok: true };
  } catch (e: unknown) {
    if (e instanceof ApiError) {
      return { ok: false, error: extractErrorMessage(e.body) };
    }
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function unbindDir(id: string): Promise<boolean> {
  try {
    await api.delete(`/index/unbind?id=${encodeURIComponent(id)}`);
    return true;
  } catch {
    return false;
  }
}

function extractErrorMessage(body: string): string {
  try {
    const parsed = JSON.parse(body) as { error?: string };
    return parsed.error?.trim() || body;
  } catch {
    return body;
  }
}
