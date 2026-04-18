/** Reader · 条目全文 + 批注 overlay · 挂在 Drawer 内 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button } from './Button';
import { toast } from './Toast';
import { getItem } from '../hooks/useItems';
import type { DecryptedItem } from '../hooks/useItems';
import {
  listAnnotations,
  createAnnotation,
  deleteAnnotation,
  analyzeByAI,
  PRESET_TAGS,
} from '../hooks/useAnnotations';
import type { Annotation, AnnotationAngle } from '../hooks/useAnnotations';

const AI_ANGLES: Array<{ key: AnnotationAngle; emoji: string; label: string }> = [
  { key: 'risk', emoji: '⚠', label: '风险' },
  { key: 'outdated', emoji: '⏰', label: '过时' },
  { key: 'highlights', emoji: '✨', label: '亮点' },
  { key: 'questions', emoji: '❓', label: '疑问' },
];

export type ReaderProps = {
  itemId: string;
};

export function Reader({ itemId }: ReaderProps): JSX.Element {
  const item = useSignal<DecryptedItem | null>(null);
  const annotations = useSignal<Annotation[]>([]);
  const loading = useSignal(true);
  const selection = useSignal<{ start: number; end: number; text: string } | null>(null);
  const aiLoading = useSignal<AnnotationAngle | null>(null);

  useEffect(() => {
    loading.value = true;
    void (async () => {
      const [it, anns] = await Promise.all([
        getItem(itemId),
        listAnnotations(itemId),
      ]);
      item.value = it;
      annotations.value = anns;
      loading.value = false;
    })();
  }, [itemId]);

  function handleMouseUp(e: MouseEvent) {
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0 || !item.value) {
      selection.value = null;
      return;
    }
    const text = sel.toString();
    if (!text.trim()) {
      selection.value = null;
      return;
    }
    // 在原文里找 offset（简单 indexOf；可能有重复匹配，取 click 附近的）
    const content = item.value.content;
    const start = content.indexOf(text);
    if (start < 0) return;
    selection.value = {
      start,
      end: start + text.length,
      text,
    };
    e.stopPropagation();
  }

  async function createWith(tagKey: string, color: string) {
    const sel = selection.value;
    if (!sel || !item.value) return;
    const ann = await createAnnotation({
      item_id: item.value.id,
      start_offset: sel.start,
      end_offset: sel.end,
      snippet: sel.text,
      tag: tagKey,
      color,
    });
    if (ann) {
      annotations.value = [...annotations.value, ann];
      toast('success', `已添加 ${tagKey} 批注`);
      selection.value = null;
      window.getSelection()?.removeAllRanges();
    } else {
      toast('error', '添加失败');
    }
  }

  async function runAI(angle: AnnotationAngle) {
    if (!item.value) return;
    aiLoading.value = angle;
    const newAnns = await analyzeByAI(item.value.id, angle);
    aiLoading.value = null;
    if (newAnns.length > 0) {
      annotations.value = [...annotations.value, ...newAnns];
      toast('success', `新增 ${newAnns.length} 条 AI 批注`);
    } else {
      toast('info', 'AI 未找到匹配片段');
    }
  }

  async function removeAnnotation(id: string) {
    const ok = await deleteAnnotation(id);
    if (ok) {
      annotations.value = annotations.value.filter((a) => a.id !== id);
    } else {
      toast('error', '删除失败');
    }
  }

  if (loading.value) {
    return <div style={{ color: 'var(--color-text-secondary)' }}>加载中…</div>;
  }
  if (!item.value) {
    return <div style={{ color: 'var(--color-error)' }}>条目不存在</div>;
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-4)' }}>
      {/* 头信息 */}
      <header>
        <h2
          style={{
            fontSize: 'var(--text-xl)',
            fontWeight: 600,
            margin: 0,
            marginBottom: 'var(--space-1)',
          }}
        >
          {item.value.title || '(无标题)'}
        </h2>
        <div
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            display: 'flex',
            gap: 'var(--space-3)',
          }}
        >
          <span>{item.value.source_type}</span>
          {item.value.domain && <span>· {item.value.domain}</span>}
          <span>· {new Date(item.value.created_at).toLocaleString()}</span>
        </div>
      </header>

      {/* AI 4 角度分析按钮 */}
      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: 'var(--space-2)',
          padding: 'var(--space-3)',
          background: 'var(--color-bg)',
          borderRadius: 'var(--radius-md)',
        }}
      >
        <span
          style={{
            alignSelf: 'center',
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
          }}
        >
          💰 AI 分析（本地 LLM）：
        </span>
        {AI_ANGLES.map((a) => (
          <Button
            key={a.key}
            size="sm"
            variant={aiLoading.value === a.key ? 'primary' : 'ghost'}
            onClick={() => runAI(a.key)}
            disabled={aiLoading.value !== null}
            loading={aiLoading.value === a.key}
          >
            {a.emoji} {a.label}
          </Button>
        ))}
      </div>

      {/* 选中时浮出的 tag bar */}
      {selection.value && (
        <div
          className="fade-slide-in"
          style={{
            position: 'sticky',
            top: 0,
            padding: 'var(--space-3)',
            background: 'var(--color-surface)',
            border: '1px solid var(--color-accent)',
            borderRadius: 'var(--radius-md)',
            boxShadow: 'var(--shadow-md)',
            display: 'flex',
            alignItems: 'center',
            gap: 'var(--space-2)',
            flexWrap: 'wrap',
            zIndex: 5,
          }}
        >
          <span style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>
            为选中文本添加批注：
          </span>
          {PRESET_TAGS.map((t) => (
            <button
              key={t.key}
              type="button"
              onClick={() => void createWith(t.key, t.color)}
              style={{
                padding: '4px 10px',
                background: t.color,
                color: 'white',
                border: 'none',
                borderRadius: 'var(--radius-sm)',
                fontSize: 'var(--text-xs)',
                cursor: 'pointer',
                fontWeight: 500,
              }}
            >
              {t.emoji} {t.key}
            </button>
          ))}
          <button
            type="button"
            onClick={() => {
              selection.value = null;
              window.getSelection()?.removeAllRanges();
            }}
            style={{
              marginLeft: 'auto',
              background: 'transparent',
              border: 'none',
              color: 'var(--color-text-secondary)',
              cursor: 'pointer',
              fontSize: 'var(--text-base)',
            }}
            aria-label="Dismiss"
          >
            ×
          </button>
        </div>
      )}

      {/* 内容（带批注高亮） */}
      <article
        onMouseUp={handleMouseUp}
        style={{
          fontSize: 'var(--text-base)',
          lineHeight: 1.7,
          color: 'var(--color-text)',
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-word',
          userSelect: 'text',
        }}
      >
        {renderWithAnnotations(item.value.content, annotations.value)}
      </article>

      {/* 批注侧边列表 */}
      {annotations.value.length > 0 && (
        <aside
          style={{
            marginTop: 'var(--space-4)',
            padding: 'var(--space-4)',
            background: 'var(--color-bg)',
            borderRadius: 'var(--radius-md)',
          }}
        >
          <h3
            style={{
              fontSize: 'var(--text-base)',
              fontWeight: 600,
              margin: 0,
              marginBottom: 'var(--space-2)',
            }}
          >
            批注（{annotations.value.length}）
          </h3>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
            {annotations.value.map((a) => (
              <AnnotationRow
                key={a.id}
                annotation={a}
                onDelete={() => void removeAnnotation(a.id)}
              />
            ))}
          </div>
        </aside>
      )}
    </div>
  );
}

function AnnotationRow({
  annotation: a,
  onDelete,
}: {
  annotation: Annotation;
  onDelete: () => void;
}): JSX.Element {
  const tag = PRESET_TAGS.find((p) => p.key === a.tag);
  return (
    <div
      style={{
        padding: 'var(--space-2) var(--space-3)',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-sm)',
        display: 'flex',
        alignItems: 'flex-start',
        gap: 'var(--space-2)',
      }}
    >
      <span
        style={{
          padding: '2px 6px',
          background: a.color ?? tag?.color ?? 'var(--color-accent)',
          color: 'white',
          borderRadius: 'var(--radius-sm)',
          fontSize: 10,
          fontWeight: 500,
          flexShrink: 0,
        }}
      >
        {tag?.emoji ?? '·'} {a.tag}
        {a.source === 'ai' && <span style={{ marginLeft: 4, opacity: 0.8 }}>🤖</span>}
      </span>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            fontStyle: 'italic',
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          "{a.snippet}"
        </div>
        {a.note && (
          <div style={{ fontSize: 'var(--text-sm)', color: 'var(--color-text)', marginTop: 2 }}>
            {a.note}
          </div>
        )}
      </div>
      <button
        type="button"
        onClick={onDelete}
        aria-label="Delete annotation"
        style={{
          background: 'transparent',
          border: 'none',
          color: 'var(--color-text-secondary)',
          cursor: 'pointer',
          fontSize: 'var(--text-sm)',
          flexShrink: 0,
        }}
      >
        ×
      </button>
    </div>
  );
}

// 渲染内容 · 按 offset 切片，在批注区间包 <mark>
function renderWithAnnotations(
  content: string,
  annotations: Annotation[],
): JSX.Element[] {
  if (annotations.length === 0) return [<span key="plain">{content}</span>];

  // 按 start_offset 排序
  const sorted = [...annotations].sort((a, b) => a.start_offset - b.start_offset);
  const out: JSX.Element[] = [];
  let cursor = 0;

  for (const a of sorted) {
    if (a.start_offset > cursor) {
      out.push(<span key={`p-${cursor}`}>{content.slice(cursor, a.start_offset)}</span>);
    }
    const color = a.color ?? '#D4A574';
    out.push(
      <mark
        key={`a-${a.id}`}
        title={`${a.tag}${a.note ? '：' + a.note : ''}`}
        style={{
          background: hexToRgba(color, 0.25),
          borderBottom: `2px solid ${color}`,
          padding: '0 2px',
          borderRadius: 2,
        }}
      >
        {content.slice(a.start_offset, a.end_offset)}
      </mark>,
    );
    cursor = Math.max(cursor, a.end_offset);
  }
  if (cursor < content.length) {
    out.push(<span key={`tail-${cursor}`}>{content.slice(cursor)}</span>);
  }
  return out;
}

function hexToRgba(hex: string, alpha: number): string {
  const m = hex.replace('#', '').match(/.{2}/g);
  if (!m) return `rgba(94, 139, 139, ${alpha})`;
  const [r, g, b] = m.map((h) => parseInt(h, 16));
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}
