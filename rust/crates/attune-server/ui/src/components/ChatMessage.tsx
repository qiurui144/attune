/** ChatMessage · 单条对话气泡
 * 见 spec §2 L4 "Chat 流式打字" + §4 "Chat 视图"
 *
 * - user 消息：右对齐，accent 底色
 * - assistant 消息：左对齐，surface 底色 + 机器人头像，支持逐字 reveal
 * - system 消息：窄宽居中，灰色小字（提示 / 错误）
 * - 引用 chip：点击触发 drawer (citation)
 */

import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import type { Message } from '../store/signals';
import { drawerContent } from '../store/signals';
import { t } from '../i18n';

export type ChatMessageProps = {
  message: Message;
  /** 流式打字效果（仅首次显示的 assistant 消息） */
  stream?: boolean;
};

const STREAM_MS_PER_CHAR = 15;

export function ChatMessage({ message: m, stream = false }: ChatMessageProps): JSX.Element {
  if (m.role === 'system') return <SystemMessage content={m.content} />;
  if (m.role === 'user') return <UserBubble content={m.content} />;
  return <AssistantBubble message={m} stream={stream} />;
}

// ── User 气泡 ────────────────────────────────────────────────
function UserBubble({ content }: { content: string }): JSX.Element {
  return (
    <div
      className="fade-slide-in"
      style={{
        display: 'flex',
        justifyContent: 'flex-end',
        padding: 'var(--space-2) 0',
      }}
    >
      <div
        style={{
          maxWidth: '78%',
          padding: 'var(--space-3) var(--space-4)',
          background: 'var(--color-accent)',
          color: 'white',
          borderRadius: 'var(--radius-lg)',
          borderBottomRightRadius: 'var(--radius-sm)',
          fontSize: 'var(--text-base)',
          lineHeight: 'var(--leading-normal)',
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-word',
        }}
      >
        {content}
      </div>
    </div>
  );
}

// ── Assistant 气泡 ───────────────────────────────────────────
function AssistantBubble({
  message: m,
  stream,
}: {
  message: Message;
  stream: boolean;
}): JSX.Element {
  const [revealedLen, setRevealedLen] = useState(stream ? 0 : m.content.length);

  useEffect(() => {
    if (!stream) {
      setRevealedLen(m.content.length);
      return;
    }
    let i = 0;
    const id = setInterval(() => {
      i += 2; // 每 tick 2 字符，快感
      if (i >= m.content.length) {
        setRevealedLen(m.content.length);
        clearInterval(id);
      } else {
        setRevealedLen(i);
      }
    }, STREAM_MS_PER_CHAR);
    return () => clearInterval(id);
  }, [m.content, stream]);

  const displayed = m.content.slice(0, revealedLen);
  const streaming = revealedLen < m.content.length;

  return (
    <div
      className="fade-slide-in"
      style={{
        display: 'flex',
        gap: 'var(--space-3)',
        padding: 'var(--space-2) 0',
      }}
    >
      <div
        aria-hidden="true"
        style={{
          width: 32,
          height: 32,
          borderRadius: '50%',
          background: 'var(--color-surface-hover)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          fontSize: 16,
          flexShrink: 0,
        }}
      >
        🌿
      </div>
      <div style={{ flex: 1, minWidth: 0, maxWidth: '90%' }}>
        <div
          style={{
            padding: 'var(--space-3) var(--space-4)',
            background: 'var(--color-surface)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-lg)',
            borderBottomLeftRadius: 'var(--radius-sm)',
            fontSize: 'var(--text-base)',
            color: 'var(--color-text)',
            lineHeight: 'var(--leading-normal)',
            whiteSpace: 'pre-wrap',
            wordBreak: 'break-word',
          }}
        >
          {displayed}
          {streaming && <TypingCaret />}
        </div>
        {m.citations && m.citations.length > 0 && !streaming && (
          <CitationRow citations={m.citations} />
        )}
      </div>
    </div>
  );
}

function TypingCaret(): JSX.Element {
  return (
    <span
      aria-hidden="true"
      className="blink"
      style={{
        display: 'inline-block',
        width: 2,
        height: '1em',
        background: 'var(--color-accent)',
        marginLeft: 2,
        verticalAlign: 'text-bottom',
      }}
    />
  );
}

// ── 引用 chip 行 ─────────────────────────────────────────────
function CitationRow({
  citations,
}: {
  citations: NonNullable<Message['citations']>;
}): JSX.Element {
  return (
    <div
      style={{
        display: 'flex',
        flexWrap: 'wrap',
        gap: 'var(--space-2)',
        marginTop: 'var(--space-2)',
      }}
    >
      <span
        style={{
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-secondary)',
          alignSelf: 'center',
        }}
      >
        {t('chat.citation.label')}
      </span>
      {citations.map((c, i) => (
        <button
          key={`${c.item_id}-${i}`}
          type="button"
          onClick={() =>
            (drawerContent.value = {
              type: 'citation',
              itemId: c.item_id,
              snippet: c.title,
            })
          }
          className="interactive"
          style={{
            padding: '2px var(--space-2)',
            background: 'var(--color-bg)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-sm)',
            fontSize: 'var(--text-xs)',
            color: 'var(--color-accent)',
            cursor: 'pointer',
            maxWidth: 240,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
          title={c.title}
        >
          {c.title}
          {c.relevance > 0 && (
            <span
              style={{
                marginLeft: 4,
                color: 'var(--color-text-secondary)',
                fontSize: 10,
              }}
            >
              {Math.round(c.relevance * 100)}%
            </span>
          )}
        </button>
      ))}
    </div>
  );
}

// ── System 消息（窄宽灰字居中） ──────────────────────────────
function SystemMessage({ content }: { content: string }): JSX.Element {
  return (
    <div
      className="fade-in"
      style={{
        padding: 'var(--space-2) 0',
        display: 'flex',
        justifyContent: 'center',
      }}
    >
      <div
        style={{
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-secondary)',
          padding: 'var(--space-2) var(--space-3)',
          background: 'var(--color-bg)',
          borderRadius: 'var(--radius-sm)',
          border: '1px dashed var(--color-border)',
          maxWidth: '80%',
          textAlign: 'center',
        }}
      >
        {content}
      </div>
    </div>
  );
}
