/** Chat 视图 · Phase 5 完整实现
 *
 * 布局：
 *   ┌────────────────────────────────┐
 *   │ 顶栏：会话标题 + 模型 chip        │
 *   ├────────────────────────────────┤
 *   │ 消息流（滚动）                   │
 *   │  (空态时 EmptyState + sample)    │
 *   ├────────────────────────────────┤
 *   │ ChatInput + Token chip + Send   │
 *   └────────────────────────────────┘
 */

import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { EmptyState, ChatMessage, ChatInput } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import {
  activeSessionId,
  messages,
  chatSessions,
  settings,
  currentView,
  settingsInitialTab,
} from '../store/signals';
import {
  loadSession,
  sendMessage,
  clearActiveSession,
  consumeSkipNextSessionLoad,
} from '../hooks/useChat';
import { patchSettings } from '../hooks/useSettings';
import type { Message } from '../store/signals';

// 各 provider 已知可选模型（与 SettingsView 的 LLM_PRESETS 对齐）。
// chip 内切换只改 model（同 provider / endpoint / key），跨 provider 走 Settings。
const PROVIDER_MODELS: Record<string, string[]> = {
  ollama: ['auto', 'qwen2.5:3b', 'qwen2.5:1.5b', 'llama3.2:3b'],
  openai: ['gpt-4o-mini', 'gpt-4o'],
  deepseek: ['deepseek-chat', 'deepseek-reasoner'],
  qwen: ['qwen-plus', 'qwen-turbo', 'qwen-max'],
  gemini: ['gemini-1.5-flash', 'gemini-1.5-pro'],
  anthropic: ['claude-3-5-haiku-latest', 'claude-3-5-sonnet-latest'],
  attune_pro: [],
};

export function ChatView(): JSX.Element {
  const currentSid = activeSessionId.value;
  const session = currentSid
    ? chatSessions.value.find((s) => s.id === currentSid)
    : null;

  // 跟随 activeSessionId 加载 session 消息。刚发送后的 session 回填会触发本 effect，
  // 但内存消息（含 acp_flow live trace）已是最新且更完整 —— 跳过那一次重载避免冲掉。
  useEffect(() => {
    if (currentSid) {
      if (consumeSkipNextSessionLoad()) return;
      void loadSession(currentSid);
    } else {
      messages.value = [];
    }
  }, [currentSid]);

  return (
    <div
      style={{
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
      }}
    >
      <ChatHeader title={session?.title ?? t('chat.new_session_title')} model={getCurrentModel()} />
      <MessageList />
      <ChatInput
        onSend={async (text) => {
          await sendMessage(text);
        }}
        isLocal={isLlmLocal()}
      />
    </div>
  );
}

// Minor 3.1 修复：从 settings 读当前模型而非硬编码
function getCurrentModel(): string {
  const s = settings.value;
  const llm = s?.llm as { model?: string } | undefined;
  const model = llm?.model?.trim();
  if (!model || model === 'auto') {
    return t('chat.model.auto');
  }
  return model;
}

// 从 settings 判断当前 LLM 是否本地（Ollama / K3 → true；cloud provider → false）
// settings 未加载时返回 null（未知），TokenChip 据此显示"—"而非误报"本地"
function isLlmLocal(): boolean | null {
  const s = settings.value;
  if (!s) return null;
  const llm = s.llm as { provider?: string } | undefined;
  if (!llm?.provider) return null;
  // cloud providers: openai / anthropic / gemini / deepseek / qwen / attune_pro / custom
  const cloudProviders = ['openai', 'anthropic', 'gemini', 'deepseek', 'qwen', 'attune_pro', 'custom'];
  return !cloudProviders.includes(llm.provider);
}

// ── Chat 顶栏 ────────────────────────────────────────────────
function ChatHeader({ title, model }: { title: string; model: string }): JSX.Element {
  return (
    <header
      style={{
        padding: 'var(--space-3) var(--space-5)',
        borderBottom: '1px solid var(--color-border)',
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-3)',
        background: 'var(--color-surface)',
      }}
    >
      <h1
        style={{
          flex: 1,
          margin: 0,
          fontSize: 'var(--text-base)',
          fontWeight: 500,
          color: 'var(--color-text)',
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
        }}
      >
        {title}
      </h1>
      <ModelChip model={model} />
    </header>
  );
}

function ModelChip({ model }: { model: string }): JSX.Element {
  const open = useSignal(false);
  const s = settings.value;
  const llm = (s?.llm as { provider?: string; model?: string } | undefined) ?? {};
  const provider = llm.provider ?? '';
  const currentModel = llm.model?.trim() || '';

  // 当前 provider 的候选模型 + 当前已配置模型（去重）
  const candidates = (() => {
    const list = PROVIDER_MODELS[provider] ?? [];
    const set = new Set<string>(list);
    if (currentModel) set.add(currentModel);
    return [...set];
  })();

  async function pickModel(m: string): Promise<void> {
    open.value = false;
    if (m === currentModel) return;
    const ok = await patchSettings({ llm: { ...llm, model: m } });
    toast(ok ? 'success' : 'error',
      ok ? t('chat.model.switched', { model: m }) : t('chat.model.switch_failed'));
  }

  function goSettings(): void {
    open.value = false;
    settingsInitialTab.value = 'ai';
    currentView.value = 'settings';
  }

  return (
    <div style={{ position: 'relative' }}>
      <button
        type="button"
        className="interactive"
        aria-haspopup="menu"
        aria-expanded={open.value}
        style={{
          padding: '4px var(--space-3)',
          background: 'var(--color-bg)',
          border: '1px solid var(--color-border)',
          borderRadius: 'var(--radius-md)',
          fontSize: 'var(--text-xs)',
          fontFamily: 'var(--font-mono)',
          color: 'var(--color-text-secondary)',
          cursor: 'pointer',
          display: 'inline-flex',
          alignItems: 'center',
          gap: 'var(--space-1)',
        }}
        onClick={() => (open.value = !open.value)}
        aria-label={t('chat.model.change')}
      >
        <span aria-hidden="true">🧠</span>
        <span>{model}</span>
        <span aria-hidden="true" style={{ fontSize: 10 }}>
          ▾
        </span>
      </button>
      {open.value && (
        <div
          role="menu"
          className="fade-slide-in"
          style={{
            position: 'absolute',
            top: 'calc(100% + var(--space-1))',
            right: 0,
            minWidth: 200,
            background: 'var(--color-surface)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-md)',
            boxShadow: 'var(--shadow-lg)',
            padding: 'var(--space-1) 0',
            zIndex: 20,
          }}
        >
          {candidates.length === 0 ? (
            <div
              style={{
                padding: '6px var(--space-3)',
                fontSize: 'var(--text-xs)',
                color: 'var(--color-text-secondary)',
              }}
            >
              {t('chat.model.none')}
            </div>
          ) : (
            candidates.map((m) => (
              <ModelMenuItem
                key={m}
                label={m}
                active={m === currentModel}
                onClick={() => void pickModel(m)}
              />
            ))
          )}
          <div style={{ height: 1, background: 'var(--color-border)', margin: 'var(--space-1) 0' }} />
          <ModelMenuItem label={t('chat.model.configure')} active={false} onClick={goSettings} />
        </div>
      )}
    </div>
  );
}

function ModelMenuItem({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      className="interactive"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-2)',
        width: '100%',
        padding: '6px var(--space-3)',
        background: active ? 'var(--color-surface-hover)' : 'transparent',
        border: 'none',
        color: 'var(--color-text)',
        fontSize: 'var(--text-sm)',
        fontFamily: 'var(--font-mono)',
        textAlign: 'left',
        cursor: 'pointer',
      }}
      onMouseEnter={(e) => (e.currentTarget.style.background = 'var(--color-surface-hover)')}
      onMouseLeave={(e) => (e.currentTarget.style.background = active ? 'var(--color-surface-hover)' : 'transparent')}
    >
      <span aria-hidden="true" style={{ width: 14, opacity: active ? 1 : 0 }}>✓</span>
      <span style={{ flex: 1 }}>{label}</span>
    </button>
  );
}

// ── 消息流 ───────────────────────────────────────────────────
function MessageList(): JSX.Element {
  const msgs = messages.value;
  const scrollRef = useRef<HTMLDivElement | null>(null);

  // 新消息到达时自动滚到底部
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [msgs.length]);

  if (msgs.length === 0) {
    return (
      <div style={{ flex: 1, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
        <EmptyState
          icon="💬"
          title={t('empty.chat.title')}
          description={t('empty.chat.desc')}
          examples={[
            t('chat.sample.summarize_recent'),
            t('chat.sample.search_topic'),
            t('chat.sample.last_topic'),
          ]}
          onExampleClick={(ex) => {
            void sendMessage(ex);
          }}
        />
      </div>
    );
  }

  // 只有最后一条 assistant 消息启用流式（避免历史消息重播）
  const lastIdx = msgs.length - 1;
  const lastMsg = msgs[lastIdx]!;
  const streamLast = lastMsg.role === 'assistant' &&
    (Date.now() - new Date(lastMsg.created_at).getTime()) < 3_000;

  return (
    <div
      ref={scrollRef}
      style={{
        flex: 1,
        overflow: 'auto',
        padding: 'var(--space-5) var(--space-6)',
        background: 'var(--color-bg)',
      }}
    >
      <div style={{ maxWidth: 900, margin: '0 auto' }}>
        {msgs.map((m: Message, i: number) => (
          <ChatMessage
            key={m.id}
            message={m}
            stream={streamLast && i === lastIdx}
          />
        ))}
      </div>
    </div>
  );
}

// 供 Sidebar 的"新对话"按钮触发
export { clearActiveSession };
