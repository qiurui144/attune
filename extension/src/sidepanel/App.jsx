import { h, render } from 'preact';
import { useState } from 'preact/hooks';
import SearchPage from './pages/SearchPage.jsx';
import TimelinePage from './pages/TimelinePage.jsx';
import StatusPage from './pages/StatusPage.jsx';
import { MSG, sendToWorker } from '../shared/messages.js';
import './sidepanel.css';

const TABS = [
  { id: 'search', label: '搜索' },
  { id: 'timeline', label: '时间线' },
  { id: 'status', label: '状态' },
];

const PAGE_MAP = { search: SearchPage, timeline: TimelinePage, status: StatusPage };

function SavePageBar() {
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState('');

  const handleSave = async () => {
    setSaving(true);
    setMsg('');
    try {
      const result = await sendToWorker(MSG.CAPTURE_PAGE);
      if (result?.status === 'ok') {
        setMsg('✓ 已保存到知识库');
      } else if (result?.status === 'duplicate') {
        setMsg('已存在（重复）');
      } else {
        setMsg(result?.error || '保存失败');
      }
    } catch (err) {
      setMsg('保存失败：' + err.message);
    }
    setSaving(false);
    setTimeout(() => setMsg(''), 3000);
  };

  return (
    <div class="sp-savebar">
      <button class="sp-savebar__btn" onClick={handleSave} disabled={saving}>
        {saving ? '保存中…' : '📌 保存当前页面'}
      </button>
      {msg && <span class="sp-savebar__msg">{msg}</span>}
    </div>
  );
}

function App() {
  const [tab, setTab] = useState('search');
  const Page = PAGE_MAP[tab];

  return (
    <div class="sp-container">
      <SavePageBar />
      <div class="sp-tabs">
        {TABS.map((t) => (
          <button
            key={t.id}
            class={`sp-tab${tab === t.id ? ' sp-tab--active' : ''}`}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div class="sp-page">
        <Page />
      </div>
    </div>
  );
}

render(<App />, document.getElementById('app'));

