// extension/src/sidepanel/pages/FilePage.jsx
import { h } from 'preact';
import { useState, useRef } from 'preact/hooks';
import { api } from '../../shared/api.js';
import { MSG } from '../../shared/messages.js';

const ALLOWED_TYPES = ['.pdf', '.docx', '.md', '.txt', '.py', '.js', '.ts'];
const SESSION_ID_KEY = 'npu_session_id';

function getSessionId() {
  let sid = sessionStorage.getItem(SESSION_ID_KEY);
  if (!sid) {
    sid = Math.random().toString(36).slice(2);
    sessionStorage.setItem(SESSION_ID_KEY, sid);
  }
  return sid;
}

export default function FilePage() {
  const [files, setFiles] = useState([]);   // [{uid, name, id, status, chunks}]
  const [dragging, setDragging] = useState(false);
  const inputRef = useRef(null);

  async function uploadFile(file) {
    const ext = '.' + file.name.split('.').pop().toLowerCase();
    if (!ALLOWED_TYPES.includes(ext)) {
      alert(`不支持的格式：${ext}。支持：${ALLOWED_TYPES.join(' ')}`);
      return;
    }
    const uid = crypto.randomUUID();  // 唯一 ID，避免同名文件混淆
    setFiles((prev) => [...prev, { uid, name: file.name, id: null, status: 'uploading', chunks: 0 }]);
    try {
      // NOTE: api.uploadFile() 由 Task 11 (api.js 更新) 添加，此处假设已存在
      const result = await api.uploadFile(file, getSessionId());
      setFiles((prev) =>
        prev.map((f) =>
          f.uid === uid   // 通过 uid 匹配，而非 name
            ? { ...f, id: result.id, status: 'done', chunks: result.chunks_queued }
            : f,
        ),
      );
      // 通知 worker 记录会话上传 ID（用于搜索加权）
      if (typeof chrome !== 'undefined' && chrome.runtime) {
        chrome.runtime.sendMessage({ type: MSG.FILE_UPLOADED, item_id: result.id });
      }
    } catch (err) {
      setFiles((prev) =>
        prev.map((f) =>
          f.uid === uid ? { ...f, status: 'error' } : f,  // 通过 uid 匹配
        ),
      );
    }
  }

  function handleDrop(e) {
    e.preventDefault();
    setDragging(false);
    Array.from(e.dataTransfer.files).forEach(uploadFile);
  }

  async function handleDelete(id) {
    if (!id) return;
    try {
      await api.deleteItem(id);
      setFiles((prev) => prev.filter((f) => f.id !== id));
    } catch { /* ignore */ }
  }

  return (
    <div class="fp-container">
      <div
        class={`fp-dropzone${dragging ? ' fp-dropzone--active' : ''}`}
        onDragOver={(e) => { e.preventDefault(); setDragging(true); }}
        onDragLeave={() => setDragging(false)}
        onDrop={handleDrop}
        onClick={() => inputRef.current?.click()}
      >
        <span>拖拽文件到此处，或点击选择</span>
        <span class="fp-hint">支持：PDF DOCX MD TXT Python JS TS</span>
        <input
          ref={inputRef}
          type="file"
          accept={ALLOWED_TYPES.join(',')}
          multiple
          style="display:none"
          onChange={(e) => Array.from(e.target.files).forEach(uploadFile)}
        />
      </div>

      {files.length > 0 && (
        <ul class="fp-list">
          {files.map((f) => (
            <li key={f.uid} class="fp-item">
              <span class={`fp-status fp-status--${f.status}`}>
                {f.status === 'uploading' ? '上传中...' : f.status === 'done' ? '✓' : '✗'}
              </span>
              <span class="fp-name">{f.name}</span>
              {f.status === 'done' && (
                <span class="fp-meta">已处理（{f.chunks} 个段落）</span>
              )}
              {f.id && (
                <button class="fp-btn-delete" onClick={() => handleDelete(f.id)}>删除</button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
