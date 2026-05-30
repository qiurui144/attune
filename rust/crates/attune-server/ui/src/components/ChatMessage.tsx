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
import type { Message, AcpFlow, AcpFlowStatus } from '../store/signals';
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
        {m.acp_flow && !streaming && <AcpFlowPanel flow={m.acp_flow} />}
      </div>
    </div>
  );
}

// ── ACP-5 自主流转块 ─────────────────────────────────────────
// 后端 chat 响应附 acp_flow（flow_id + status + 每步 trace）。让用户看到
// 自主流转真发生了，而非静默 —— free 用户 entitlement 拦截显示为 degraded/skipped。
const FLOW_STATUS_STYLE: Record<
  AcpFlowStatus,
  { bg: string; fg: string; border: string }
> = {
  complete: { bg: 'rgba(34,197,94,0.12)', fg: '#16a34a', border: 'rgba(34,197,94,0.4)' },
  partial: { bg: 'rgba(234,179,8,0.12)', fg: '#ca8a04', border: 'rgba(234,179,8,0.4)' },
  degraded: { bg: 'rgba(234,179,8,0.12)', fg: '#ca8a04', border: 'rgba(234,179,8,0.4)' },
  aborted: { bg: 'rgba(239,68,68,0.12)', fg: '#dc2626', border: 'rgba(239,68,68,0.4)' },
};

function flowStatusLabel(status: AcpFlowStatus): string {
  // 未知 status 兜底（后端将来加新 status 时不至于显示 raw key）
  const key = `chat.flow.status.${status}`;
  const label = t(key);
  return label === key ? status : label;
}

function AcpFlowPanel({ flow }: { flow: AcpFlow }): JSX.Element {
  const [open, setOpen] = useState(false);
  const style = FLOW_STATUS_STYLE[flow.status] ?? FLOW_STATUS_STYLE.partial;

  return (
    <div
      style={{
        marginTop: 'var(--space-2)',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-1)',
      }}
    >
      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: 'var(--space-2)',
          alignItems: 'center',
        }}
      >
        <span
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
          }}
        >
          ⚙ {t('chat.flow.label')}
        </span>
        <span
          style={{
            padding: '2px var(--space-2)',
            background: 'var(--color-bg)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-sm)',
            fontSize: 'var(--text-xs)',
            fontFamily: 'var(--font-mono, monospace)',
            color: 'var(--color-text)',
          }}
        >
          {flow.flow_id}
        </span>
        <span
          style={{
            padding: '2px var(--space-2)',
            background: style.bg,
            border: `1px solid ${style.border}`,
            borderRadius: 'var(--radius-sm)',
            fontSize: 'var(--text-xs)',
            fontWeight: 600,
            color: style.fg,
          }}
        >
          {flowStatusLabel(flow.status)}
        </span>
        {flow.steps.length > 0 && (
          <button
            type="button"
            className="interactive"
            onClick={() => setOpen((v) => !v)}
            aria-expanded={open}
            style={{
              padding: '2px var(--space-2)',
              background: 'transparent',
              border: 'none',
              fontSize: 'var(--text-xs)',
              color: 'var(--color-accent)',
              cursor: 'pointer',
            }}
          >
            {open ? t('chat.flow.collapse') : t('chat.flow.expand')} ({flow.steps.length})
          </button>
        )}
      </div>
      {open && flow.steps.length > 0 && (
        <ul
          aria-label={t('chat.flow.steps_aria')}
          style={{
            listStyle: 'none',
            margin: 0,
            padding: 'var(--space-2)',
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--space-2)',
            background: 'var(--color-bg)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-sm)',
          }}
        >
          {flow.steps.map((s, i) => (
            <AcpFlowStepRow key={`${s.agent_id}-${i}`} step={s} />
          ))}
        </ul>
      )}
    </div>
  );
}

function AcpFlowStepRow({
  step,
}: {
  step: AcpFlow['steps'][number];
}): JSX.Element {
  // 三态：ran+degraded=降级 / ran=已执行 / !ran=已跳过（entitlement 拦截等）
  const stateLabel = step.degraded
    ? t('chat.flow.step.degraded')
    : step.ran
      ? t('chat.flow.step.ran')
      : t('chat.flow.step.skipped');
  const dotColor = step.degraded
    ? '#ca8a04'
    : step.ran
      ? '#16a34a'
      : 'var(--color-text-secondary)';
  return (
    <li
      style={{
        display: 'flex',
        gap: 'var(--space-2)',
        fontSize: 'var(--text-xs)',
        lineHeight: 'var(--leading-normal)',
      }}
    >
      <span
        aria-hidden="true"
        style={{
          width: 8,
          height: 8,
          borderRadius: '50%',
          background: dotColor,
          flexShrink: 0,
          marginTop: 5,
        }}
      />
      <div style={{ minWidth: 0 }}>
        <span style={{ color: 'var(--color-text)', fontWeight: 600 }}>
          {step.agent_id}
        </span>
        <span style={{ color: dotColor, marginLeft: 6 }}>· {stateLabel}</span>
        {step.note && (
          <div
            style={{
              color: 'var(--color-text-secondary)',
              marginTop: 2,
              wordBreak: 'break-word',
            }}
          >
            {step.note}
          </div>
        )}
      </div>
    </li>
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
