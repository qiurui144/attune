/** useBrowserLogin · 浏览器登入协助会话管理（GAP-A）
 *
 * 安全契约：凭据 / 会话 storage_state **绝不**经此 hook 回显。connect 触发登入
 * （scan 探测 / 人在回路 login）；list 只拿脱敏会话（provider + 域名 + 状态）；
 * delete 移除。auto 模式默认关 + 需付费会员（后端 403），UI 默认不启用。
 */
import { api, ApiError } from '../store/api';

/** 脱敏登入会话视图 —— 无 secret / storage_state 字段（与后端 SessionView 对齐）。 */
export type BrowserLoginSession = {
  id: string;
  provider: string;
  source_name: string;
  domain: string;
  status: string;
  created_at: string;
  updated_at: string;
};

export type ConnectMode = 'scan' | 'login';

export type ConnectInput = {
  source_name: string;
  entry_url: string;
  mode: ConnectMode;
  /** auto 表单识别填充 —— 默认 false（人在回路为主）；启用需付费会员。 */
  auto?: boolean;
  wait_seconds?: number;
};

export type ConnectResult = {
  ok: boolean;
  /** login 成功捕获了会话。 */
  captured?: boolean;
  /** 探测 / login 的脱敏 outcome（success / needs-human / restricted / ...）。 */
  outcome?: string;
  needs_login?: boolean;
  error?: string;
};

type ListResponse = { sessions: BrowserLoginSession[] };

export async function listBrowserLoginSessions(): Promise<BrowserLoginSession[]> {
  try {
    const res = await api.get<ListResponse>('/browser-login/sessions');
    return res.sessions ?? [];
  } catch {
    return [];
  }
}

export async function connectBrowserLogin(input: ConnectInput): Promise<ConnectResult> {
  try {
    const res = await api.post<Record<string, unknown>>('/browser-login/connect', input);
    return {
      ok: true,
      captured: res.captured === true,
      outcome: typeof res.outcome === 'string' ? res.outcome : undefined,
      needs_login: res.needs_login === true,
    };
  } catch (e: unknown) {
    return { ok: false, error: toErrorMessage(e) };
  }
}

export async function deleteBrowserLoginSession(id: string): Promise<boolean> {
  try {
    await api.delete(`/browser-login/sessions/${encodeURIComponent(id)}`);
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
