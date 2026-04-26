/** MainShell · Sidebar + Main area + DrawerHost
 * 见 spec §4 "整体结构"
 */

import type { JSX } from 'preact';
import { Sidebar } from './Sidebar';
import { DrawerHost } from './DrawerHost';
import {
  ChatView,
  ItemsView,
  ProjectsView,
  RemoteView,
  KnowledgeView,
  SettingsView,
} from '../views';
import { currentView } from '../store/signals';

export function MainShell(): JSX.Element {
  const view = currentView.value;

  return (
    <div
      style={{
        height: '100vh',
        display: 'flex',
        background: 'var(--color-bg)',
        overflow: 'hidden',
      }}
    >
      <Sidebar />
      <main
        style={{
          flex: 1,
          overflow: 'auto',
          background: 'var(--color-surface)',
        }}
      >
        {view === 'chat' && <ChatView />}
        {view === 'items' && <ItemsView />}
        {view === 'projects' && <ProjectsView />}
        {view === 'remote' && <RemoteView />}
        {view === 'knowledge' && <KnowledgeView />}
        {view === 'settings' && <SettingsView />}
      </main>
      <DrawerHost />
    </div>
  );
}
