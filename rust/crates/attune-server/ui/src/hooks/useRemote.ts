/** useRemote · 已绑定目录（本地 + WebDAV）管理 */
import { api } from '../store/api';

export type BoundDir = {
  id: string;
  path: string;
  recursive: boolean;
  file_types: string;
  last_scan?: string;
  kind: 'local' | 'webdav';
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

export async function bindLocalDir(path: string): Promise<boolean> {
  try {
    await api.post('/index/bind', { path, recursive: true });
    return true;
  } catch {
    return false;
  }
}

export type WebdavInput = {
  url: string;
  username: string;
  password: string;
  remote_path: string;
};

export async function bindWebdav(input: WebdavInput): Promise<boolean> {
  try {
    await api.post('/index/bind-remote', input);
    return true;
  } catch {
    return false;
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
