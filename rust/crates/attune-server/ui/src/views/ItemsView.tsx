/** Items 视图 · Phase 6 · 真实列表 + 筛选 + Reader drawer 触发 */

import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
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
              label: t('items.empty.bind_folder'),
              onClick: () => toast('info', t('items.empty.bind_folder_toast')),
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
          placeholder={t('items.search.placeholder')}
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
              {st === 'all' ? t('items.source.all') : st}
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
            {t('items.no_match')}
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
        {t('items.summary', { filtered: filtered.value.length, total: items.value.length })}
      </div>
    </div>
  );
}

function ItemsHeader(): JSX.Element {
  const fileInputRef = useRef<HTMLInputElement>(null);
  const uploading = useSignal(false);

  const onUpload = async (files: FileList | null) => {
    if (!files || files.length === 0) return;
    uploading.value = true;
    let successCount = 0;
    let failCount = 0;
    for (const file of Array.from(files)) {
      const form = new FormData();
      form.append('file', file);
      try {
        const resp = await fetch('/api/v1/upload', {
          method: 'POST',
          body: form,
          headers: {
            Authorization: `Bearer ${sessionStorage.getItem('attune_token') ?? ''}`,
          },
        });
        if (resp.ok) {
          successCount++;
        } else {
          failCount++;
        }
      } catch {
        failCount++;
      }
    }
    uploading.value = false;
    if (successCount > 0) {
      toast('success', t('items.upload.success', { count: successCount }));
      void loadItems(100, 0);
    }
    if (failCount > 0) {
      toast('error', t('items.upload.fail', { count: failCount }));
    }
  };

  return (
    <header
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
      }}
    >
      <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0 }}>
        {`📄 ${t('sidebar.nav.items')}`}
      </h2>
      <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
        <input
          ref={fileInputRef}
          type="file"
          multiple
          accept=".pdf,.md,.txt,.docx,.png,.jpg,.jpeg"
          style={{ display: 'none' }}
          onChange={(e) => void onUpload((e.target as HTMLInputElement).files)}
        />
        <Button
          variant="secondary"
          size="sm"
          disabled={uploading.value}
          onClick={() => fileInputRef.current?.click()}
        >
          {uploading.value ? t('items.upload.uploading') : t('items.upload.button')}
        </Button>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => void loadItems(100, 0)}
        >
          {t('items.refresh')}
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
          {it.title || t('items.untitled')}
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
          if (confirm(t('items.delete.confirm', { title: it.title || t('items.untitled') }))) {
            void deleteItem(it.id).then((ok) => {
              if (ok) toast('success', t('items.delete.success'));
              else toast('error', t('items.delete.fail'));
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
    if (diff < 86_400_000) return t('items.date.today');
    if (diff < 2 * 86_400_000) return t('items.date.yesterday');
    if (diff < 7 * 86_400_000) return t('items.date.days_ago', { days: Math.floor(diff / 86_400_000) });
    return d.toLocaleDateString();
  } catch {
    return iso.slice(0, 10);
  }
}
