import { render } from 'preact';
import { App } from './App';
import './styles/global.css';

const rootEl = document.getElementById('app');
if (!rootEl) {
  throw new Error('Root element #app not found');
}

render(<App />, rootEl);
