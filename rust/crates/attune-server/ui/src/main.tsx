import { render } from 'preact';
import { App } from './App';
import './styles/global.css';

// file-drop 处理统一在 App.tsx(preact 树内,可用 toast/store)。

const rootEl = document.getElementById('app');
if (!rootEl) {
  throw new Error('Root element #app not found');
}

render(<App />, rootEl);
