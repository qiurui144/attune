import { render } from 'preact';
import { App } from './App';
import { t } from './i18n';
import './styles/global.css';

const rootEl = document.getElementById('app');
if (!rootEl) {
  throw new Error('Root element #app not found');
}

// Tauri 桌面壳的 file-drop 事件桥
// 浏览器模式下 __TAURI_INTERNALS__ 不存在，listener 不会挂载
if (typeof window !== 'undefined' && (window as any).__TAURI_INTERNALS__) {
  import('@tauri-apps/api/event')
    .then(({ listen }) => {
      listen<string[]>('attune-file-drop', (event) => {
        const paths = event.payload || [];
        console.log('[attune-desktop] dropped files:', paths);
        // Sprint 1 接 store.uploadFromPaths(paths)
        // 当前先 alert 让用户验证桥通
        if (paths.length > 0) {
          alert(
            t('app.tauri.dropped_files', {
              count: paths.length,
              paths: paths.slice(0, 3).join('\n'),
            }),
          );
        }
      }).catch((err) => {
        console.warn('failed to attach attune-file-drop listener:', err);
      });
    })
    .catch((err) => {
      console.warn('failed to import @tauri-apps/api/event:', err);
    });
}

render(<App />, rootEl);
