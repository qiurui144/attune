/** Items 视图 · Phase 6 · 真实列表 + 筛选 + Reader drawer 触发 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal, useComputed } from '@preact/signals';
import { Button, EmptyState } from '../components';
import { t } from '../i18n';
import { items, drawerContent } from '../store/signals';
import type { Item } from '../store/signals';
import { loadItems, deleteItem } from '../hooks/useItems';
import { toast } from '../components/Toast';

export function ItemsView(): JSX.Element {
  const filterSource = useSignal<string>('all');
  const search = useSignal('');

  useEffect(() => {
    void loadItems(100, 0);
  }, []);

  const filtered = useComputed(() => {
    const src = filterSource.value;
    const q = search.value.trim().toLowerCase();
    return items.value.filter((it) => {
      if (src !== 'all' && it.source_type !== src) return false;
      if (q && !it.title.toLowerCase().includes(q)) return false;
      return true;
    });
  });

  const sourceTypes = useComputed(() => {
    const set = new Set<string>();
    items.value.forEach((it) => set.add(it.source_type));
    return ['all', ...Array.from(set).sort()];
  });

  if (items.value.length === 0) {
    return (
      <div style={{ padding: 'var(--space-5)', height: '100%' }}>
        <ItemsHeader />
        <EmptyState
          icon="📂"
          title={t('empty.items.title')}
          description={t('empty.items.desc')}
          actions={[
            {
              label: '绑定文件夹',
              onClick: () => toast('info', '跳转到远程绑定（Phase 6.3 接入）'),
              variant: 'primary',
            },
          ]}
        />
      </div>
    );
  }

  return (
    <div
      style={{
        padding: 'var(--space-5)',
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-4)',
      }}
    >
      <ItemsHeader />

      {/* 筛选条 */}
      <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'center' }}>
        <input
          type="text"
          placeholder="🔍 按标题搜索…"
          value={search.value}
          onInput={(e) => (search.value = e.currentTarget.value)}
          style={{
            flex: 1,
            padding: 'var(--space-2) var(--space-3)',
            fontSize: 'var(--text-sm)',
            background: 'var(--color-surface)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-md)',
            outline: 'none',
          }}
        />
        <select
          value={filterSource.value}
          onChange={(e) => (filterSource.value = e.currentTarget.value)}
          style={{
            padding: 'var(--space-2) var(--space-3)',
            fontSize: 'var(--text-sm)',
            background: 'var(--color-surface)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-md)',
          }}
        >
          {sourceTypes.value.map((st) => (
            <option key={st} value={st}>
              {st === 'all' ? '全部来源' : st}
            </option>
          ))}
        </select>
      </div>

      {/* 条目列表 */}
      <div
        style={{
          flex: 1,
          overflow: 'auto',
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-2)',
        }}
      >
        {filtered.value.length === 0 ? (
          <div
            style={{
              padding: 'var(--space-6)',
              textAlign: 'center',
              color: 'var(--color-text-secondary)',
            }}
          >
            没有匹配的条目
          </div>
        ) : (
          filtered.value.map((it) => <ItemRow key={it.id} item={it} />)
        )}
      </div>

      <div
        style={{
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-secondary)',
          textAlign: 'right',
        }}
      >
        共 {filtered.value.length} 条 · 加载 {items.value.length} 条
      </div>
    </div>
  );
}

function ItemsHeader(): JSX.Element {
  return (
    <header
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
      }}
    >
      <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0 }}>
        📄 条目
      </h2>
      <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => toast('info', '上传文件（Phase 6 续集）')}
        >
          上传文件
        </Button>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => void loadItems(100, 0)}
        >
          ⟳ 刷新
        </Button>
      </div>
    </header>
  );
}

function ItemRow({ item: it }: { item: Item }): JSX.Element {
  return (
    <div
      className="interactive"
      onClick={() =>
        (drawerContent.value = { type: 'reader', itemId: it.id })
      }
      style={{
        padding: 'var(--space-3) var(--space-4)',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        cursor: 'pointer',
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-3)',
      }}
      onMouseEnter={(e) =>
        (e.currentTarget.style.background = 'var(--color-surface-hover)')
      }
      onMouseLeave={(e) => (e.currentTarget.style.background = 'var(--color-surface)')}
    >
      <span
        aria-hidden="true"
        style={{
          padding: '2px 8px',
          fontSize: 10,
          background: 'var(--color-bg)',
          border: '1px solid var(--color-border)',
          borderRadius: 'var(--radius-sm)',
          color: 'var(--color-text-secondary)',
          fontFamily: 'var(--font-mono)',
          flexShrink: 0,
        }}
      >
        {it.source_type}
      </span>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontSize: 'var(--text-base)',
            color: 'var(--color-text)',
            fontWeight: 500,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          {it.title || '(无标题)'}
        </div>
        {it.domain && (
          <div
            style={{
              fontSize: 'var(--text-xs)',
              color: 'var(--color-text-secondary)',
              marginTop: 2,
            }}
          >
            {it.domain}
          </div>
        )}
      </div>
      <time
        dateTime={it.created_at}
        style={{
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-secondary)',
          flexShrink: 0,
        }}
      >
        {formatDate(it.created_at)}
      </time>
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          if (confirm(`删除条目 "${it.title}"？`)) {
            void deleteItem(it.id).then((ok) => {
              if (ok) toast('success', '已删除');
              else toast('error', '删除失败');
            });
          }
        }}
        aria-label="Delete"
        style={{
          background: 'transparent',
          border: 'none',
          color: 'var(--color-text-secondary)',
          cursor: 'pointer',
          fontSize: 'var(--text-base)',
          padding: '4px 6px',
          borderRadius: 'var(--radius-sm)',
        }}
        onMouseEnter={(e) => (e.currentTarget.style.color = 'var(--color-error)')}
        onMouseLeave={(e) =>
          (e.currentTarget.style.color = 'var(--color-text-secondary)')
        }
      >
        ×
      </button>
    </div>
  );
}

function formatDate(iso: string): string {
  try {
    const d = new Date(iso);
    const now = Date.now();
    const diff = now - d.getTime();
    if (diff < 86_400_000) return '今天';
    if (diff < 2 * 86_400_000) return '昨天';
    if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} 天前`;
    return d.toLocaleDateString();
  } catch {
    return iso.slice(0, 10);
  }
}
