/** Knowledge 视图 · Phase 4 占位 */

import type { JSX } from 'preact';
import { EmptyState } from '../components';
import { t } from '../i18n';

export function KnowledgeView(): JSX.Element {
  return (
    <div style={{ padding: 'var(--space-5)', height: '100%' }}>
      <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0, marginBottom: 'var(--space-4)' }}>
        {`📊 ${t('sidebar.nav.knowledge')}`}
      </h2>
      <EmptyState
        icon="📊"
        title={t('knowledge.empty.title')}
        description={t('knowledge.empty.desc')}
      />
    </div>
  );
}
