/** ChatInput · 输入框 + Token chip + 发送
 * 见 spec §2 L4 · §4 "Chat 视图 · 输入 + Token chip"
 *
 * - auto-grow textarea（2 行起步，最多 8 行）
 * - Cmd+Enter / Ctrl+Enter 发送；单 Enter 换行
 * - Token chip 实时估算
 * - submitting 时 disable + spinner
 */

import type { JSX } from 'preact';
import { useState, useRef, useEffect } from 'preact/hooks';
import { estimateTokens } from '../hooks/useChat';
import { lastCostEstimate } from '../store/signals';
import { t } from '../i18n';

export type ChatInputProps = {
  onSend: (text: string) => Promise<void> | void;
  disabled?: boolean;
  placeholder?: string;
  /** 本地模型显示"~本地"，云端显示估算花费；null = settings 未加载，显示"—" */
  isLocal?: boolean | null;
};

const MAX_HEIGHT_LINES = 8;

export function ChatInput({
  onSend,
  disabled = false,
  placeholder,
  isLocal = null,
}: ChatInputProps): JSX.Element {
  const [text, setText] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  const tokens = estimateTokens(text);
  const canSend = text.trim().length > 0 && !submitting && !disabled;
  const resolvedPlaceholder =
    placeholder && placeholder.includes('.')
      ? t(placeholder)
      : (placeholder ?? t('chat.input.placeholder'));

  // Auto-grow
  useEffect(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    ta.style.height = 'auto';
    const lineH = 24;
    const newHeight = Math.min(ta.scrollHeight, lineH * MAX_HEIGHT_LINES);
    ta.style.height = `${newHeight}px`;
  }, [text]);

  async function handleSend() {
    if (!canSend) return;
    const value = text;
    setText('');
    setSubmitting(true);
    try {
      await onSend(value);
    } finally {
      setSubmitting(false);
    }
  }

  function handleKeyDown(e: JSX.TargetedKeyboardEvent<HTMLTextAreaElement>) {
    // Cmd+Enter / Ctrl+Enter 发送
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void handleSend();
    }
  }

  return (
    <div
      style={{
        padding: 'var(--space-3) var(--space-5) var(--space-5)',
        borderTop: '1px solid var(--color-border)',
        background: 'var(--color-surface)',
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'flex-end',
          gap: 'var(--space-2)',
          padding: 'var(--space-2) var(--space-3)',
          background: 'var(--color-bg)',
          border: '1px solid var(--color-border)',
          borderRadius: 'var(--radius-lg)',
          transition: 'border-color var(--duration-fast) var(--ease-out)',
        }}
        onFocusCapture={(e) =>
          (e.currentTarget.style.borderColor = 'var(--color-accent)')
        }
        onBlurCapture={(e) => (e.currentTarget.style.borderColor = 'var(--color-border)')}
      >
        <textarea
          ref={textareaRef}
          value={text}
          onInput={(e) => setText(e.currentTarget.value)}
          onKeyDown={handleKeyDown}
          placeholder={resolvedPlaceholder}
          aria-label={t('chat.input.aria')}
          disabled={disabled || submitting}
          rows={1}
          style={{
            flex: 1,
            resize: 'none',
            border: 'none',
            outline: 'none',
            background: 'transparent',
            color: 'var(--color-text)',
            fontFamily: 'var(--font-sans)',
            fontSize: 'var(--text-base)',
            lineHeight: '24px',
            padding: 'var(--space-1) 0',
            maxHeight: 24 * MAX_HEIGHT_LINES,
          }}
        />
        <TokenChip tokens={tokens} isLocal={isLocal} />
        <SendButton onClick={handleSend} disabled={!canSend} loading={submitting} />
      </div>
      <div
        style={{
          marginTop: 'var(--space-2)',
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-disabled)',
          textAlign: 'right',
        }}
      >
        <kbd
          style={{
            padding: '0 4px',
            background: 'var(--color-surface-hover)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-sm)',
            fontFamily: 'var(--font-mono)',
          }}
        >
          ⌘↵
        </kbd>{' '}
        {t('shortcut.send')}
      </div>
    </div>
  );
}

function TokenChip({ tokens, isLocal }: { tokens: number; isLocal: boolean | null }): JSX.Element {
  const display =
    tokens === 0
      ? ''
      : tokens >= 1000
        ? `~${(tokens / 1000).toFixed(1)}K`
        : `~${tokens}`;

  // 成本契约诚信：只展示真实费率推导的 $；无真实单价时绝不编造美元数。
  // 后端响应携带的精确 input 单价；优先于 isLocal prop（后者在首次发送前可能为 null）
  const lastCost = lastCostEstimate.value;
  const effectiveIsLocal = lastCost ? lastCost.is_local : isLocal;
  let suffix: string;
  let suffixTitle: string | undefined;
  if (effectiveIsLocal === null) {
    // settings 未加载，provider 未知 → 显示"—"而非误报本地/费用
    suffix = t('chat.token.unknown');
  } else if (effectiveIsLocal) {
    suffix = t('chat.token.local');
  } else if (lastCost?.input_rate_per_k != null) {
    // 真实 input 单价（来自后端定价表，input/output 价差最大 5× → 用 input 价更稳）
    suffix = `~$${(tokens * lastCost.input_rate_per_k / 1000).toFixed(4)}`;
    suffixTitle = t('chat.cost.estimated_title');
  } else {
    // 已知云端但无真实单价（首次发送前 / model 不在定价表）→ 仅标"云端"，不编 $
    suffix = t('chat.token.cloud_no_rate');
    suffixTitle = t('chat.cost.no_rate_title');
  }

  return (
    <div
      aria-label={t('chat.tokens.aria', { tokens: String(tokens) })}
      title={suffixTitle}
      style={{
        fontSize: 'var(--text-xs)',
        color: 'var(--color-text-secondary)',
        fontFamily: 'var(--font-mono)',
        whiteSpace: 'nowrap',
        padding: '4px 6px',
        alignSelf: 'center',
      }}
    >
      {tokens > 0 && `${display} tok · ${suffix}`}
    </div>
  );
}

function SendButton({
  onClick,
  disabled,
  loading,
}: {
  onClick: () => void;
  disabled: boolean;
  loading: boolean;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={t('chat.send.aria')}
      className="interactive"
      style={{
        width: 32,
        height: 32,
        borderRadius: '50%',
        background: disabled ? 'var(--color-border)' : 'var(--color-accent)',
        color: 'white',
        border: 'none',
        cursor: disabled ? 'not-allowed' : 'pointer',
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        fontSize: 16,
        flexShrink: 0,
      }}
    >
      {loading ? <span className="spinner" /> : '↑'}
    </button>
  );
}
