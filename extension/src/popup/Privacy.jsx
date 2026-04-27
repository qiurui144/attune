// G5 浏览隐私控制面板 (W3 batch B, 2026-04-27)
//
// per spec docs/superpowers/specs/2026-04-27-w3-batch-b-design.md §4
//
// 功能：
//   1. per-domain whitelist 增删（默认 opt-out — 用户必须显式加域名才捕获）
//   2. 全局 Pause toggle（content script 检查后跳过捕获）
//   3. 显示已捕获信号数 + "数据仅本机不上传"提示
//   4. 清除按钮（per-domain 或全部）

import { h } from 'preact';
import { useState, useEffect } from 'preact/hooks';
import { api } from '../shared/api.js';

const styles = {
  wrap: { padding: '12px', fontFamily: 'system-ui, sans-serif', fontSize: '13px' },
  title: { margin: '0 0 8px', fontSize: '14px', fontWeight: 600 },
  status: { padding: '8px', borderRadius: '4px', background: '#f3f4f6', marginBottom: '12px', fontSize: '12px' },
  toggleRow: { display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: '12px' },
  btn: { padding: '6px 12px', border: 'none', borderRadius: '4px', cursor: 'pointer', background: '#4f46e5', color: '#fff', fontSize: '12px' },
  btnSecondary: { padding: '4px 8px', border: '1px solid #d1d5db', borderRadius: '4px', cursor: 'pointer', background: '#fff', fontSize: '12px' },
  btnDanger: { padding: '6px 12px', border: 'none', borderRadius: '4px', cursor: 'pointer', background: '#ef4444', color: '#fff', fontSize: '12px' },
  list: { listStyle: 'none', margin: 0, padding: 0, maxHeight: '120px', overflowY: 'auto' },
  listItem: { display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '4px 8px', background: '#f9fafb', borderRadius: '4px', marginBottom: '4px' },
  input: { flex: 1, padding: '6px', border: '1px solid #d1d5db', borderRadius: '4px', fontSize: '12px' },
  inputRow: { display: 'flex', gap: '6px', marginTop: '8px' },
  hint: { fontSize: '11px', color: '#6b7280', marginTop: '8px', lineHeight: 1.4 },
  empty: { padding: '12px', textAlign: 'center', color: '#9ca3af', fontSize: '12px' },
};

export function Privacy() {
  const [whitelist, setWhitelist] = useState([]);
  const [paused, setPaused] = useState(false);
  const [count, setCount] = useState(0);
  const [draft, setDraft] = useState('');

  // 加载状态
  useEffect(() => {
    chrome.storage.local.get(['browseWhitelist', 'browsePaused'], (cfg) => {
      setWhitelist(cfg.browseWhitelist || []);
      setPaused(!!cfg.browsePaused);
    });
    refreshCount();
  }, []);

  async function refreshCount() {
    try {
      const r = await api.listBrowseSignals(0);
      setCount(r.count || 0);
    } catch {
      setCount(0);
    }
  }

  function togglePause() {
    const next = !paused;
    chrome.storage.local.set({ browsePaused: next });
    setPaused(next);
  }

  function addDomain() {
    const d = draft.trim().toLowerCase();
    if (!d || d.length < 3 || whitelist.includes(d)) {
      setDraft('');
      return;
    }
    const next = [...whitelist, d];
    chrome.storage.local.set({ browseWhitelist: next });
    setWhitelist(next);
    setDraft('');
  }

  function removeDomain(d) {
    const next = whitelist.filter((x) => x !== d);
    chrome.storage.local.set({ browseWhitelist: next });
    setWhitelist(next);
  }

  async function clearAll() {
    if (!confirm('清除所有已捕获的浏览信号？此操作不可撤销。')) return;
    try {
      await api.clearBrowseSignals();
      await refreshCount();
    } catch (e) {
      alert('清除失败: ' + (e?.message || e));
    }
  }

  async function clearForDomain(d) {
    try {
      await api.clearBrowseSignals(d);
      await refreshCount();
    } catch (e) {
      alert('清除失败: ' + (e?.message || e));
    }
  }

  return (
    <div style={styles.wrap}>
      <h3 style={styles.title}>Browse Signal Privacy</h3>
      <div style={styles.status}>
        状态：{paused ? '⏸ 已暂停' : '🟢 active'} · 已捕获 {count} 条
      </div>
      <div style={styles.toggleRow}>
        <span>全局暂停（应用所有域名）</span>
        <button style={paused ? styles.btnSecondary : styles.btn} onClick={togglePause}>
          {paused ? 'Resume' : 'Pause'}
        </button>
      </div>

      <h4 style={{ margin: '12px 0 6px', fontSize: '13px' }}>Per-domain Whitelist</h4>
      {whitelist.length === 0 ? (
        <div style={styles.empty}>默认不捕获任何域名 — 添加你想跟踪学习的网站</div>
      ) : (
        <ul style={styles.list}>
          {whitelist.map((d) => (
            <li key={d} style={styles.listItem}>
              <span>{d}</span>
              <span>
                <button style={{ ...styles.btnSecondary, marginRight: '4px' }} onClick={() => clearForDomain(d)}>清除</button>
                <button style={styles.btnSecondary} onClick={() => removeDomain(d)}>删除</button>
              </span>
            </li>
          ))}
        </ul>
      )}
      <div style={styles.inputRow}>
        <input
          style={styles.input}
          type="text"
          placeholder="example.com"
          value={draft}
          onInput={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && addDomain()}
        />
        <button style={styles.btn} onClick={addDomain}>添加</button>
      </div>

      <div style={{ marginTop: '12px', display: 'flex', justifyContent: 'flex-end' }}>
        <button style={styles.btnDanger} onClick={clearAll}>清除所有已捕获</button>
      </div>
      <div style={styles.hint}>
        💡 默认 opt-out：你必须显式添加域名才会被捕获。<br />
        🔒 银行 / 医疗 / 政府登录页 / 密码管理器始终被硬黑名单屏蔽。<br />
        📍 所有数据仅存本机，不上传任何远端服务。
      </div>
    </div>
  );
}
