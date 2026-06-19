/** useThirdPartyAccounts · 通用第三方账号凭据管理（弱腿增量波 C / 能力 B）
 *
 * 安全契约：凭据（secret）只在「添加」时单向入参，列表 / 响应**永不回显 secret**。
 * 建议卡不在此 hook —— 它走既有 useSuggestions.ts（develop 零成本规则引擎）。本 hook
 * 只管账号 CRUD（GET/POST/DELETE /accounts）；连接源数量由后端 suggestions route
 * 读入 SignalContext 驱动 ConnectSource 卡。
 */
import { api, ApiError } from '../store/api';

/** 后端已知 provider 白名单（与 attune-core KNOWN_PROVIDERS 对齐）。 */
export const PROVIDERS = ['webdav', 'imap', 'rss', 'git', 'other'] as const;
export type Provider = (typeof PROVIDERS)[number];

/** 脱敏账户视图 —— 无 secret 字段（与后端 AccountResponse 对齐）。 */
export type ThirdPartyAccount = {
  id: string;
  provider: string;
  label: string;
  username: string;
  endpoint: string;
  status: string;
  created_at: string;
  updated_at: string;
};

export type AddAccountInput = {
  provider: Provider;
  label: string;
  username: string;
  endpoint: string;
  /** 明文凭据 —— 仅添加时入参，提交后前端立即清空，不缓存。 */
  secret: string;
};

type ListAccountsResponse = { accounts: ThirdPartyAccount[] };

export async function listThirdPartyAccounts(): Promise<ThirdPartyAccount[]> {
  try {
    const res = await api.get<ListAccountsResponse>('/accounts');
    return res.accounts ?? [];
  } catch {
    return [];
  }
}

export type AddResult = { ok: boolean; error?: string };

export async function addThirdPartyAccount(input: AddAccountInput): Promise<AddResult> {
  try {
    await api.post('/accounts', input);
    return { ok: true };
  } catch (e: unknown) {
    return { ok: false, error: toErrorMessage(e) };
  }
}

export async function deleteThirdPartyAccount(id: string): Promise<boolean> {
  try {
    await api.delete(`/accounts/${encodeURIComponent(id)}`);
    return true;
  } catch {
    return false;
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
