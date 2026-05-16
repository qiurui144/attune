/** DrawerHost · 监听 drawerContent signal，挂载对应内容的 Drawer
 * 见 spec §4 "Slide-in Drawer（侧滑抽屉 · 单层）"
 */

import type { JSX } from 'preact';
import { Drawer, Reader, AgentResultPanel } from '../components';
import { t } from '../i18n';
import { drawerContent } from '../store/signals';

export function DrawerHost(): JSX.Element | null {
  const content = drawerContent.value;
  if (!content) return null;

  return (
    <Drawer
      open
      onClose={() => (drawerContent.value = null)}
      title={titleFor(content)}
      defaultWidth={640}
    >
      {renderContent(content)}
    </Drawer>
  );
}

function titleFor(c: NonNullable<typeof drawerContent.value>): string {
  switch (c.type) {
    case 'reader':
      return 'Reader';
    case 'citation':
      return t('drawer.citation.title');
    case 'annotation-composer':
      return t('drawer.annotation.title');
    case 'help':
      return t('drawer.help.title', { topic: c.topic });
    case 'agent-result':
      return c.result.title;
  }
}

function renderContent(c: NonNullable<typeof drawerContent.value>): JSX.Element {
  switch (c.type) {
    case 'reader':
      return <Reader itemId={c.itemId} />;
    case 'citation':
      return (
        <div>
          <p style={{ color: 'var(--color-text-secondary)', marginBottom: 'var(--space-3)' }}>
            Item: <code>{c.itemId}</code>
          </p>
          <blockquote
            style={{
              padding: 'var(--space-3)',
              background: 'var(--color-bg)',
              borderLeft: '3px solid var(--color-accent)',
              fontSize: 'var(--text-sm)',
              color: 'var(--color-text)',
              margin: 0,
            }}
          >
            {c.snippet}
          </blockquote>
        </div>
      );
    case 'annotation-composer':
      return (
        <div>
          <p style={{ color: 'var(--color-text-secondary)' }}>Offset: {c.offset}</p>
          <blockquote style={{ marginTop: 'var(--space-3)', fontStyle: 'italic' }}>
            {c.selection}
          </blockquote>
          <p style={{ marginTop: 'var(--space-3)' }}>
            {t('drawer.annotation.composer_wip')}
          </p>
        </div>
      );
    case 'help':
      return (
        <div>
          <p>{t('drawer.help.wip')}</p>
        </div>
      );
    case 'agent-result':
      return <AgentResultPanel result={c.result} />;
  }
}
