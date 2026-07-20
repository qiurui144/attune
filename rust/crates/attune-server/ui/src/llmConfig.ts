/** Shared, side-effect-free LLM configuration helpers for Settings and Chat. */

export type LlmPresetKey =
  | 'custom'
  | 'deepseek'
  | 'qwen'
  | 'glm'
  | 'kimi'
  | 'baichuan'
  | 'local_scheduler'
  | 'openai';

export interface LlmPreset {
  labelKey: string;
  endpoint: string;
  model: string;
}

export const LLM_PRESETS: Record<LlmPresetKey, LlmPreset> = {
  custom: { labelKey: 'settings.ai.llm.preset.custom', endpoint: '', model: '' },
  deepseek: {
    labelKey: 'settings.ai.llm.preset.deepseek',
    endpoint: 'https://api.deepseek.com/v1',
    model: 'deepseek-chat',
  },
  qwen: {
    labelKey: 'settings.ai.llm.preset.qwen',
    endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    model: 'qwen-plus',
  },
  glm: {
    labelKey: 'settings.ai.llm.preset.glm',
    endpoint: 'https://open.bigmodel.cn/api/paas/v4',
    model: 'glm-4-plus',
  },
  kimi: {
    labelKey: 'settings.ai.llm.preset.kimi',
    endpoint: 'https://api.moonshot.cn/v1',
    model: 'moonshot-v1-8k',
  },
  baichuan: {
    labelKey: 'settings.ai.llm.preset.baichuan',
    endpoint: 'https://api.baichuan-ai.com/v1',
    model: 'Baichuan4-Turbo',
  },
  local_scheduler: {
    labelKey: 'settings.ai.llm.preset.local_scheduler',
    endpoint: 'http://127.0.0.1:8090/v1',
    model: 'llm-chat',
  },
  openai: {
    labelKey: 'settings.ai.llm.preset.openai',
    endpoint: 'https://api.openai.com/v1',
    model: 'gpt-4o-mini',
  },
};

export const LLM_MODEL_OPTIONS = Array.from(
  new Set(Object.values(LLM_PRESETS).map((preset) => preset.model).filter(Boolean)),
);

const LOCAL_PROVIDER_NAMES = new Set([
  'local',
  'local_scheduler',
  'edge_scheduler',
  'scheduler_native',
]);

function parseHttpUrl(endpoint: string): URL | null {
  try {
    const url = new URL(endpoint.trim());
    return url.protocol === 'http:' || url.protocol === 'https:' ? url : null;
  } catch {
    return null;
  }
}

function normalizedHost(url: URL): string {
  return url.hostname.toLowerCase().replace(/^\[/, '').replace(/\]$/, '');
}

function parseIpv4(host: string): number[] | null {
  if (!/^\d{1,3}(?:\.\d{1,3}){3}$/.test(host)) return null;
  const octets = host.split('.').map(Number);
  return octets.every((octet) => octet >= 0 && octet <= 255) ? octets : null;
}

function isLocalNetworkHost(host: string): boolean {
  if (host === 'localhost') return true;

  const ipv4 = parseIpv4(host);
  if (ipv4) {
    const [a, b] = ipv4;
    return a === 0
      || a === 10
      || a === 127
      || (a === 169 && b === 254)
      || (a === 172 && b >= 16 && b <= 31)
      || (a === 192 && b === 168);
  }

  if (host === '::' || host === '::1') return true;
  const firstHextet = Number.parseInt(host.split(':', 1)[0] ?? '', 16);
  return Number.isFinite(firstHextet)
    && ((firstHextet & 0xfe00) === 0xfc00 || (firstHextet & 0xffc0) === 0xfe80);
}

/** Mirrors the server's exact-localhost + non-public IP destination classifier. */
export function isLocalNetworkEndpoint(endpoint: string): boolean {
  const url = parseHttpUrl(endpoint);
  return url !== null && isLocalNetworkHost(normalizedHost(url));
}

/** True only for a syntactically valid HTTP(S) destination outside local/private networks. */
export function isCloudNetworkEndpoint(endpoint: string): boolean {
  const url = parseHttpUrl(endpoint);
  return url !== null && !isLocalNetworkHost(normalizedHost(url));
}

/** Mirrors the server's legacy scheduler heuristic: local/private destination on port 8090. */
export function isSchedulerEndpoint(endpoint: string): boolean {
  const url = parseHttpUrl(endpoint);
  if (!url || !isLocalNetworkHost(normalizedHost(url))) return false;
  const port = url.port ? Number(url.port) : url.protocol === 'https:' ? 443 : 80;
  return port === 8090;
}

/** Convert a scheduler-native base into the OpenAI-compatible base used by `/llm/test`. */
export function schedulerOpenAiEndpoint(endpoint: string): string {
  return `${schedulerNativeBase(endpoint)}/v1`;
}

/** Strip the OpenAI adapter suffix returned by scheduler discovery before persistence. */
export function schedulerNativeBase(endpoint: string): string {
  return normalizedEndpoint(endpoint).replace(/\/v1$/i, '');
}

function normalizedEndpoint(value: string): string {
  return value.trim().replace(/\/+$/, '');
}

export function detectLlmPreset(provider: string, endpoint: string): LlmPresetKey {
  const normalizedProvider = provider.trim().toLowerCase();
  if (LOCAL_PROVIDER_NAMES.has(normalizedProvider) || isSchedulerEndpoint(endpoint)) {
    return 'local_scheduler';
  }

  const normalized = normalizedEndpoint(endpoint);
  for (const key of ['deepseek', 'qwen', 'glm', 'kimi', 'baichuan', 'openai'] as const) {
    if (
      normalized !== ''
      && normalized.toLowerCase() === normalizedEndpoint(LLM_PRESETS[key].endpoint).toLowerCase()
    ) {
      return key;
    }
  }
  return 'custom';
}

/**
 * Classify the effective chat destination. `openai_compat` is a protocol, not a
 * locality signal: it can point at api.openai.com or a scheduler on localhost.
 */
export function isLlmConfigLocal(
  provider: string | null | undefined,
  endpoint: string | null | undefined,
): boolean | null {
  const normalizedProvider = provider?.trim().toLowerCase() ?? '';
  if (LOCAL_PROVIDER_NAMES.has(normalizedProvider)) return true;
  if (!endpoint?.trim() || !parseHttpUrl(endpoint)) return null;
  return isLocalNetworkEndpoint(endpoint);
}

const MODEL_CATALOG_BY_HOST: Record<string, string> = {
  'api.openai.com': 'openai',
  'api.deepseek.com': 'deepseek',
  'dashscope.aliyuncs.com': 'qwen',
  'generativelanguage.googleapis.com': 'gemini',
};

export function modelCatalogKey(provider: string, endpoint: string): string {
  const normalizedProvider = provider.trim().toLowerCase();
  if (normalizedProvider !== 'openai_compat') return normalizedProvider;
  const url = parseHttpUrl(endpoint);
  if (!url) return normalizedProvider;
  return MODEL_CATALOG_BY_HOST[normalizedHost(url)] ?? normalizedProvider;
}
