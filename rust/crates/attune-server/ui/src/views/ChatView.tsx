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
import { EmptyState, ChatMessage, ChatInput } from '../components';
import { t } from '../i18n';
import { activeSessionId, messages, chatSessions, settings } from '../store/signals';
import { loadSession, sendMessage, clearActiveSession } from '../hooks/useChat';
import type { Message } from '../store/signals';

export function ChatView(): JSX.Element {
  const currentSid = activeSessionId.value;
  const session = currentSid
    ? chatSessions.value.find((s) => s.id === currentSid)
    : null;

  // 跟随 activeSessionId 加载 session 消息
  useEffect(() => {
    if (currentSid) {
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
      <ChatHeader title={session?.title ?? '新对话'} model={getCurrentModel()} />
      <MessageList />
      <ChatInput
        onSend={async (text) => {
          await sendMessage(text);
        }}
        isLocal
      />
    </div>
  );
}

// Minor 3.1 修复：从 settings 读当前模型而非硬编码
function getCurrentModel(): string {
  const s = settings.value;
  const llm = s?.llm as { model?: string } | undefined;
  return llm?.model ?? 'qwen2.5:3b';
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
  return (
    <button
      type="button"
      className="interactive"
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
      onClick={() => {}}
      aria-label="Change model"
    >
      <span aria-hidden="true">🧠</span>
      <span>{model}</span>
      <span aria-hidden="true" style={{ fontSize: 10 }}>
        ▾
      </span>
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
            '帮我总结最近上传的文件',
            '搜索关于 XXX 的所有内容',
            '我上次讨论了什么话题？',
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
