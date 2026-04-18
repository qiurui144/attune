/**
 * Attune API 客户端 · 带重试 + 请求 ID + 认证 token
 * 见 spec §5 "API 层" + §7 "请求重试矩阵"
 */

const API_BASE = '/api/v1';
const TOKEN_KEY = 'attune_token';

// ── 重试策略矩阵 ─────────────────────────────────────────────────
export type RetryPolicy = {
  /** 最大尝试次数（含首次） */
  attempts: number;
  /** 指数回退起始 ms */
  initialDelay: number;
  /** 上限 ms */
  maxDelay: number;
  /** 哪些 HTTP 状态码触发重试；默认 5xx + 网络错误 */
  retryOnStatus?: number[];
};

export const RETRY_POLICIES = {
  /** 幂等读 —— 可 5 次重试 */
  idempotentRead: {
    attempts: 5,
    initialDelay: 100,
    maxDelay: 1600,
    retryOnStatus: [500, 502, 503, 504],
  } as RetryPolicy,
  /** 非幂等写 —— 3 次 + 请求 ID 去重 */
  nonIdempotentWrite: {
    attempts: 3,
    initialDelay: 200,
    maxDelay: 2000,
    retryOnStatus: [500, 502, 503, 504],
  } as RetryPolicy,
  /** 破坏性操作 —— 不自动重试 */
  destructive: {
    attempts: 1,
    initialDelay: 0,
    maxDelay: 0,
  } as RetryPolicy,
  /** 心跳 —— 不重试，调用方驱动 */
  heartbeat: {
    attempts: 1,
    initialDelay: 0,
    maxDelay: 0,
  } as RetryPolicy,
} as const;

// ── 错误类型 ─────────────────────────────────────────────────────
export class ApiError extends Error {
  constructor(
    public status: number,
    public body: string,
    public requestId: string,
  ) {
    super(`HTTP ${status}: ${body}`);
    this.name = 'ApiError';
  }
}

export class NetworkError extends Error {
  constructor(
    public cause: unknown,
    public requestId: string,
  ) {
    super(`Network error: ${String(cause)}`);
    this.name = 'NetworkError';
  }
}

// ── 认证 token 管理 ──────────────────────────────────────────────
export function setToken(token: string): void {
  sessionStorage.setItem(TOKEN_KEY, token);
}

export function getToken(): string | null {
  return sessionStorage.getItem(TOKEN_KEY);
}

export function clearToken(): void {
  sessionStorage.removeItem(TOKEN_KEY);
}

// ── 核心 apiCall ─────────────────────────────────────────────────
export type ApiCallOptions = RequestInit & {
  retry?: RetryPolicy;
  signal?: AbortSignal;
};

export async function apiCall<T>(
  path: string,
  options: ApiCallOptions = {},
): Promise<T> {
  const policy = options.retry ?? RETRY_POLICIES.idempotentRead;
  const reqId = crypto.randomUUID();
  let attempt = 0;
  let delay = policy.initialDelay;

  while (attempt < policy.attempts) {
    attempt++;
    try {
      const res = await fetchWithAuth(path, options, reqId);
      if (res.ok) {
        // 204 No Content 返回 null 强制转 T（调用方自保）
        if (res.status === 204) return null as T;
        return (await res.json()) as T;
      }

      // 401 → 清 token 让调用层跳登录
      if (res.status === 401) {
        clearToken();
        throw new ApiError(401, 'unauthorized', reqId);
      }

      // 4xx（除 401） → 不重试
      if (res.status >= 400 && res.status < 500) {
        throw new ApiError(res.status, await res.text(), reqId);
      }

      // 5xx → 看策略是否重试
      const shouldRetry =
        policy.retryOnStatus?.includes(res.status) && attempt < policy.attempts;
      if (!shouldRetry) {
        throw new ApiError(res.status, await res.text(), reqId);
      }
    } catch (e) {
      if (e instanceof ApiError) throw e;
      // 网络错误（fetch 失败）
      if (attempt >= policy.attempts) {
        throw new NetworkError(e, reqId);
      }
    }

    // 指数回退
    await sleep(delay);
    delay = Math.min(delay * 2, policy.maxDelay);
  }

  throw new NetworkError(new Error('retry exhausted'), reqId);
}

async function fetchWithAuth(
  path: string,
  options: ApiCallOptions,
  reqId: string,
): Promise<Response> {
  const token = getToken();
  const headers = new Headers(options.headers);
  headers.set('X-Request-Id', reqId);
  if (!headers.has('Content-Type') && options.body) {
    headers.set('Content-Type', 'application/json');
  }
  if (token) headers.set('Authorization', `Bearer ${token}`);

  return fetch(`${API_BASE}${path}`, { ...options, headers });
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// ── 便捷方法 ─────────────────────────────────────────────────────
export const api = {
  get<T>(path: string, retry?: RetryPolicy): Promise<T> {
    return apiCall<T>(path, { method: 'GET', retry: retry ?? RETRY_POLICIES.idempotentRead });
  },
  post<T>(path: string, body?: unknown, retry?: RetryPolicy): Promise<T> {
    const opts: ApiCallOptions = {
      method: 'POST',
      retry: retry ?? RETRY_POLICIES.nonIdempotentWrite,
    };
    if (body !== undefined) opts.body = JSON.stringify(body);
    return apiCall<T>(path, opts);
  },
  patch<T>(path: string, body: unknown, retry?: RetryPolicy): Promise<T> {
    return apiCall<T>(path, {
      method: 'PATCH',
      body: JSON.stringify(body),
      retry: retry ?? RETRY_POLICIES.nonIdempotentWrite,
    });
  },
  delete<T>(path: string, retry?: RetryPolicy): Promise<T> {
    return apiCall<T>(path, {
      method: 'DELETE',
      retry: retry ?? RETRY_POLICIES.destructive,
    });
  },
};
