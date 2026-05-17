/**
 * Attune 全局 state · Preact Signals
 * 见 spec §5 "State 管理"
 *
 * 设计原则：
 *   - 应用级 + UI 级 + 业务级三类 signal 分组
 *   - computed 派生数据优于手动维护
 *   - 所有持久化状态（theme / locale / sidebarCollapsed）启动时从 localStorage 水合
 */

import { signal, computed } from '@preact/signals';

// ── 应用级（vault 生命周期） ─────────────────────────────────────
export type VaultState = 'sealed' | 'locked' | 'unlocked' | 'unknown';
export const vaultState = signal<VaultState>('unknown');

export type WizardState = {
  complete: boolean;
  currentStep: 1 | 2 | 3 | 4 | 5;
  llmConfigured: boolean;
  hardwareApplied: boolean;
  firstDataChosen: 'folder' | 'import' | 'skip' | null;
};
export const wizardState = signal<WizardState | null>(null);

// ── UI 级（视图 / 主题 / 布局） ──────────────────────────────────
export type View =
  | 'chat'
  | 'items'
  | 'projects'
  | 'remote'
  | 'knowledge'
  | 'skills'
  | 'marketplace'  // G3 (2026-05-01): PluginHub 插件市场
  | 'settings';
export const currentView = signal<View>('chat');

export type Theme = 'light' | 'dark' | 'auto';
export const theme = signal<Theme>(loadTheme());

export const sidebarCollapsed = signal<boolean>(loadBool('attune.sidebar.collapsed', false));

export type DrawerPayload =
  | { type: 'reader'; itemId: string }
  | { type: 'citation'; itemId: string; snippet: string }
  | { type: 'annotation-composer'; itemId: string; offset: number; selection: string }
  | { type: 'help'; topic: string }
  // 变体 A · agent 结果面板（law-pro 金额计算等）
  | { type: 'agent-result'; result: import('../components/AgentResultPanel').AgentResult };
export const drawerContent = signal<DrawerPayload | null>(null);

// ── 连接层（见 store/connection.ts） ─────────────────────────────
export type ConnectionState = 'online' | 'reconnecting' | 'offline';
export const connectionState = signal<ConnectionState>('online');

// ── 业务级 ──────────────────────────────────────────────────────
export const settings = signal<Record<string, unknown> | null>(null);
export const hardware = signal<Record<string, unknown> | null>(null);
export const ollamaStatus = signal<'checking' | 'ready' | 'missing'>('checking');

// ── 会员状态 (调 /api/v1/member/state) ────────────────────────────
export type MemberStateKind = 'logged_out' | 'free' | 'paid';
export type MemberSnapshot = {
  kind: MemberStateKind;
  account_id?: string | null;
  license_id?: string | null;
  is_logged_in: boolean;
  is_paid: boolean;
};
export const memberState = signal<MemberSnapshot | null>(null);

// SettingsLocks (调 /api/v1/member/locks) — 6 字段, 决定 UI 灰显
export type LockState = 'editable' | 'locked';
export type SettingsLocksMap = {
  vault_password: LockState;
  local_folder_links: LockState;
  cloud_llm: LockState;
  plugin_install: LockState;
  plugin_uninstall: LockState;
  ocr_profiles: LockState;
};
export const settingsLocks = signal<SettingsLocksMap | null>(null);

// ── OCR Profile (调 /api/v1/ocr/profiles) ─────────────────────────
export type OcrProfile = {
  id: string;
  name: string;
  description: string;
  languages: string;
  dpi: number;
  tags: string[];
  builtin: boolean;
};
export const ocrProfiles = signal<OcrProfile[]>([]);

// ── Folder Links (调 /api/v1/folder-links GET) ───────────────────
export type FolderLink = {
  id?: string;
  path: string;
  project_id?: string;
  added_at?: string;
};
export const folderLinks = signal<FolderLink[]>([]);

export type ChatSession = {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
};
export const chatSessions = signal<ChatSession[]>([]);
export const activeSessionId = signal<string | null>(null);

export type Message = {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  citations?: Array<{ item_id: string; title: string; relevance: number }>;
  created_at: string;
};
export const messages = signal<Message[]>([]);

// ── Cost & Trigger Contract: LLM 调用费用估算（来自后端响应） ─────
export type CostEstimate = {
  tokens_in: number;
  tokens_out: number;
  cost_usd: number | null; // null = 本地模型（无 USD 计费）
  is_local: boolean;
  /** input token 单价（USD/1K），来自后端定价表；本地模型或未知模型为 null */
  input_rate_per_k: number | null;
};
/** 最近一次 chat 响应携带的 cost_estimate，供 TokenChip 展示真实费率 */
export const lastCostEstimate = signal<CostEstimate | null>(null);

export type Item = {
  id: string;
  title: string;
  source_type: string;
  domain?: string;
  created_at: string;
};
export const items = signal<Item[]>([]);

export type BackgroundTask = {
  type: string;
  task_id: string;
  progress: number;
  status: 'running' | 'done' | 'failed';
  message: string;
};
export const backgroundTasks = signal<BackgroundTask[]>([]);

// ── Sprint 1 Phase D-2: ws 推送的推荐 / workflow 完成通知 ──────────
export type RecommendationCandidate = {
  project_id: string;
  project_title: string;
  score: number;
  overlapping_entities: string[];
};

export type RecommendationPayload =
  | {
      type: 'project_recommendation';
      trigger: 'file_uploaded';
      file_id: string;
      candidates: RecommendationCandidate[];
    }
  | {
      type: 'project_recommendation';
      trigger: 'chat_keyword';
      matched_keywords?: string[];
      suggestion?: string;
    };

export type WorkflowCompletePayload = {
  type: 'workflow_complete';
  workflow_id: string;
  file_id: string;
  project_id?: string;
};

/** 队列：每条推荐作为右下角浮窗显示，用户点接受 / 忽略消失 */
export const recommendations = signal<RecommendationPayload[]>([]);

export function pushRecommendation(payload: RecommendationPayload): void {
  recommendations.value = [...recommendations.value, payload];
}

export function dismissRecommendation(index: number): void {
  recommendations.value = recommendations.value.filter((_, i) => i !== index);
}

// ── Computed ────────────────────────────────────────────────────
export const canChat = computed(
  () =>
    vaultState.value === 'unlocked' &&
    ollamaStatus.value === 'ready' &&
    connectionState.value !== 'offline',
);

export const isWizardActive = computed(
  () =>
    vaultState.value !== 'unknown' &&
    vaultState.value !== 'locked' &&
    wizardState.value?.complete !== true,
);

// ── 持久化辅助 ──────────────────────────────────────────────────
function loadTheme(): Theme {
  try {
    const v = localStorage.getItem('attune.theme') as Theme | null;
    if (v === 'light' || v === 'dark' || v === 'auto') return v;
  } catch {
    // localStorage 不可用（隐身模式 / 服务端 render） → auto
  }
  return 'auto';
}

function loadBool(key: string, defaultValue: boolean): boolean {
  try {
    const v = localStorage.getItem(key);
    if (v === 'true') return true;
    if (v === 'false') return false;
  } catch {
    /* noop */
  }
  return defaultValue;
}

// 订阅关键变化 → 持久化
theme.subscribe((v) => {
  try {
    localStorage.setItem('attune.theme', v);
    document.documentElement.setAttribute('data-theme', v);
  } catch {
    /* noop */
  }
});

sidebarCollapsed.subscribe((v) => {
  try {
    localStorage.setItem('attune.sidebar.collapsed', String(v));
  } catch {
    /* noop */
  }
});
