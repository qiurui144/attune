/** DrawerHost · 监听 drawerContent signal，挂载对应内容的 Drawer
 * 见 spec §4 "Slide-in Drawer（侧滑抽屉 · 单层）"
 */

import type { JSX } from 'preact';
import { Drawer, Reader } from '../components';
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
      return '引用原文';
    case 'annotation-composer':
      return '创建批注';
    case 'help':
      return `帮助 · ${c.topic}`;
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
            Annotation composer（Phase 5）将接入 POST /annotations。
          </p>
        </div>
      );
    case 'help':
      return (
        <div>
          <p>Help drawer（Phase 4.x）将加载 help/{c.topic}.md。</p>
        </div>
      );
  }
}
