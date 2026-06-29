/** Knowledge 视图 · 可执行知识概览 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { Button } from '../components';
import { currentView, items } from '../store/signals';
import { t } from '../i18n';
import { loadItems } from '../hooks/useItems';

export function KnowledgeView(): JSX.Element {
  useEffect(() => {
    void loadItems(100, 0);
  }, []);

  const loadedItems = items.value;
  const sources = new Set(loadedItems.map((item) => item.source_type)).size;
  const domains = new Set(loadedItems.map((item) => item.domain).filter(Boolean)).size;
  const recent = loadedItems.slice(0, 6);

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
      <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--space-3)' }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0 }}>
            {`📊 ${t('sidebar.nav.knowledge')}`}
          </h2>
          <p style={{ margin: 'var(--space-1) 0 0', color: 'var(--color-text-secondary)', fontSize: 'var(--text-sm)' }}>
            {t('knowledge.overview.desc')}
          </p>
        </div>
        <Button variant="secondary" size="sm" onClick={() => void loadItems(100, 0)}>
          {t('knowledge.action.refresh')}
        </Button>
      </div>

      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))',
          gap: 'var(--space-3)',
        }}
      >
        <Metric label={t('knowledge.metric.items')} value={loadedItems.length} />
        <Metric label={t('knowledge.metric.sources')} value={sources} />
        <Metric label={t('knowledge.metric.domains')} value={domains} />
      </div>

      <div style={{ display: 'flex', gap: 'var(--space-2)', flexWrap: 'wrap' }}>
        <Button variant="primary" size="sm" onClick={() => (currentView.value = 'items')}>
          {t('knowledge.action.items')}
        </Button>
        <Button variant="secondary" size="sm" onClick={() => (currentView.value = 'remote')}>
          {t('knowledge.action.sources')}
        </Button>
        <Button variant="secondary" size="sm" onClick={() => (currentView.value = 'projects')}>
          {t('knowledge.action.projects')}
        </Button>
      </div>

      <section
        style={{
          flex: 1,
          minHeight: 0,
          overflow: 'auto',
          borderTop: '1px solid var(--color-border)',
          paddingTop: 'var(--space-3)',
        }}
      >
        <h3 style={{ margin: '0 0 var(--space-3)', fontSize: 'var(--text-base)', fontWeight: 600 }}>
          {t('knowledge.recent.title')}
        </h3>
        {recent.length === 0 ? (
          <div style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-sm)' }}>
            {t('knowledge.recent.empty')}
          </div>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
            {recent.map((item) => (
              <button
                key={item.id}
                type="button"
                className="interactive"
                onClick={() => (currentView.value = 'items')}
                style={{
                  width: '100%',
                  padding: 'var(--space-3)',
                  background: 'var(--color-surface)',
                  border: '1px solid var(--color-border)',
                  borderRadius: 'var(--radius-md)',
                  textAlign: 'left',
                  cursor: 'pointer',
                }}
              >
                <div style={{ color: 'var(--color-text)', fontSize: 'var(--text-sm)', fontWeight: 500 }}>
                  {item.title || t('items.untitled')}
                </div>
                <div style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-xs)', marginTop: 4 }}>
                  {[item.source_type, item.domain].filter(Boolean).join(' · ')}
                </div>
              </button>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: number }): JSX.Element {
  return (
    <div
      style={{
        padding: 'var(--space-3)',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
      }}
    >
      <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)' }}>{label}</div>
      <div style={{ fontSize: 'var(--text-xl)', fontWeight: 600, color: 'var(--color-text)', marginTop: 4 }}>
        {value}
      </div>
    </div>
  );
}
