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
  SkillsView,
  MarketplaceView,
  OfficeView,
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
        background:
          'radial-gradient(120% 100% at 100% 0%, rgba(127, 165, 165, 0.12) 0%, var(--color-bg) 45%)',
        overflow: 'hidden',
      }}
    >
      <Sidebar />
      <main
        style={{
          flex: 1,
          minWidth: 0,
          overflow: 'auto',
          background: 'var(--color-surface)',
        }}
      >
        {view === 'chat' && <ChatView />}
        {view === 'items' && <ItemsView />}
        {view === 'projects' && <ProjectsView />}
        {view === 'remote' && <RemoteView />}
        {view === 'knowledge' && <KnowledgeView />}
        {view === 'skills' && <SkillsView />}
        {view === 'marketplace' && <MarketplaceView />}
        {view === 'office' && <OfficeView />}
        {view === 'settings' && <SettingsView />}
      </main>
      <DrawerHost />
    </div>
  );
}
