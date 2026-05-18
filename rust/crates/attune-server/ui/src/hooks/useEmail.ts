/** useEmail · IMAP 邮箱采集账户管理 */
import { api } from '../store/api';
import { ApiError } from '../store/api';

export type EmailAccount = {
  dir_id: string;
  host: string;
  port: number;
  username: string;
  folders: string[];
  corpus_domain: string;
  last_sync?: string;
};

export type EmailSyncStats = {
  total_documents: number;
  new_items: number;
  updated_items: number;
  skipped_items: number;
  errors: string[];
};

export type EmailActionResult = {
  ok: boolean;
  error?: string;
  stats?: EmailSyncStats;
};

export type EmailAccountInput = {
  host: string;
  port: number;
  username: string;
  password: string;
  folders: string[];
};

type ListResponse = { accounts: EmailAccount[] };
type SyncResponse = { dir_id: string; sync: EmailSyncStats };

export async function listEmailAccounts(): Promise<EmailAccount[]> {
  try {
    const res = await api.get<ListResponse>('/index/email-accounts');
    return res.accounts ?? [];
  } catch {
    return [];
  }
}

export async function addEmailAccount(input: EmailAccountInput): Promise<EmailActionResult> {
  try {
    const res = await api.post<SyncResponse>('/index/bind-email', input);
    return { ok: true, stats: res.sync };
  } catch (e: unknown) {
    return { ok: false, error: toErrorMessage(e) };
  }
}

export async function deleteEmailAccount(dirId: string): Promise<boolean> {
  try {
    await api.delete(`/index/email-accounts/${encodeURIComponent(dirId)}`);
    return true;
  } catch {
    return false;
  }
}

export async function syncEmailAccount(dirId: string): Promise<EmailActionResult> {
  try {
    const res = await api.post<SyncResponse>(
      `/index/email-accounts/${encodeURIComponent(dirId)}/sync`,
      {},
    );
    return { ok: true, stats: res.sync };
  } catch (e: unknown) {
    return { ok: false, error: toErrorMessage(e) };
  }
}

function toErrorMessage(e: unknown): string {
  if (e instanceof ApiError) {
    try {
      const parsed = JSON.parse(e.body) as { error?: string };
      return parsed.error?.trim() || e.body;
    } catch {
      return e.body;
    }
  }
  return e instanceof Error ? e.message : String(e);
}
