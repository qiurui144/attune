/** Remote 视图 · Phase 6 · 本地 + WebDAV 目录管理 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState, Modal, Input } from '../components';
import { toast } from '../components/Toast';
import {
  listBoundDirs,
  bindLocalDir,
  bindWebdav,
  unbindDir,
} from '../hooks/useRemote';
import type { BoundDir } from '../hooks/useRemote';

export function RemoteView(): JSX.Element {
  const dirs = useSignal<BoundDir[]>([]);
  const loading = useSignal(true);
  const modal = useSignal<null | 'local' | 'webdav'>(null);

  async function refresh() {
    loading.value = true;
    dirs.value = await listBoundDirs();
    loading.value = false;
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function handleUnbind(d: BoundDir) {
    if (!confirm(`解绑 ${d.path}？已索引的内容保留，但不再监听变化。`)) return;
    const ok = await unbindDir(d.id);
    if (ok) {
      toast('success', '已解绑');
      await refresh();
    } else {
      toast('error', '解绑失败');
    }
  }

  return (
    <div
      style={{
        padding: 'var(--space-5)',
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-4)',
      }}
    >
      <header
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
        }}
      >
        <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0 }}>
          🔗 远程目录
        </h2>
        <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
          <Button variant="secondary" size="sm" onClick={() => (modal.value = 'local')}>
            📂 添加本地
          </Button>
          <Button variant="primary" size="sm" onClick={() => (modal.value = 'webdav')}>
            ☁ 添加 WebDAV
          </Button>
        </div>
      </header>

      {loading.value ? (
        <div style={{ color: 'var(--color-text-secondary)' }}>加载中…</div>
      ) : dirs.value.length === 0 ? (
        <EmptyState
          icon="🔗"
          title="还没绑定任何目录"
          description="绑定本地文件夹或 WebDAV，Attune 会自动监听变化索引进知识库"
          actions={[
            { label: '添加本地文件夹', onClick: () => (modal.value = 'local'), variant: 'primary' },
            { label: '添加 WebDAV', onClick: () => (modal.value = 'webdav'), variant: 'secondary' },
          ]}
        />
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
          {dirs.value.map((d) => (
            <DirRow key={d.id} dir={d} onUnbind={() => void handleUnbind(d)} />
          ))}
        </div>
      )}

      <Modal
        open={modal.value === 'local'}
        onClose={() => (modal.value = null)}
        title="绑定本地文件夹"
      >
        <LocalForm
          onDone={async (ok) => {
            modal.value = null;
            if (ok) {
              toast('success', '已绑定，后台开始索引');
              await refresh();
            } else {
              toast('error', '绑定失败');
            }
          }}
        />
      </Modal>

      <Modal
        open={modal.value === 'webdav'}
        onClose={() => (modal.value = null)}
        title="绑定 WebDAV 远程目录"
        maxWidth={520}
      >
        <WebdavForm
          onDone={async (ok) => {
            modal.value = null;
            if (ok) {
              toast('success', '已绑定，开始首次同步');
              await refresh();
            } else {
              toast('error', 'WebDAV 绑定失败，检查 URL / 凭据');
            }
          }}
        />
      </Modal>
    </div>
  );
}

function DirRow({
  dir: d,
  onUnbind,
}: {
  dir: BoundDir;
  onUnbind: () => void;
}): JSX.Element {
  return (
    <div
      style={{
        padding: 'var(--space-3) var(--space-4)',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-3)',
      }}
    >
      <span aria-hidden="true" style={{ fontSize: 20 }}>
        {d.kind === 'webdav' ? '☁' : '📂'}
      </span>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontFamily: 'var(--font-mono)',
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text)',
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          {d.path}
        </div>
        <div
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            marginTop: 2,
          }}
        >
          {d.recursive ? '递归' : '非递归'} · 类型：{d.file_types}
          {d.last_scan && ` · 上次扫描：${new Date(d.last_scan).toLocaleString()}`}
        </div>
      </div>
      <Button variant="ghost" size="sm" onClick={onUnbind}>
        解绑
      </Button>
    </div>
  );
}

function LocalForm({
  onDone,
}: {
  onDone: (ok: boolean) => void;
}): JSX.Element {
  const path = useSignal('');
  const submitting = useSignal(false);

  async function submit() {
    if (!path.value.trim()) return;
    submitting.value = true;
    const ok = await bindLocalDir(path.value.trim());
    submitting.value = false;
    onDone(ok);
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      <Input
        label="路径"
        value={path.value}
        onInput={(e) => (path.value = e.currentTarget.value)}
        placeholder="/home/user/Documents/knowledge"
        autoFocus
        required
        hint="服务器可访问的绝对路径；Attune 会监听变化自动索引"
      />
      <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
        <Button variant="ghost" onClick={() => onDone(false)}>
          取消
        </Button>
        <Button
          variant="primary"
          onClick={submit}
          loading={submitting.value}
          disabled={!path.value.trim()}
        >
          绑定
        </Button>
      </div>
    </div>
  );
}

function WebdavForm({
  onDone,
}: {
  onDone: (ok: boolean) => void;
}): JSX.Element {
  const url = useSignal('');
  const username = useSignal('');
  const password = useSignal('');
  const remotePath = useSignal('/');
  const submitting = useSignal(false);

  async function submit() {
    submitting.value = true;
    const ok = await bindWebdav({
      url: url.value.trim(),
      username: username.value,
      password: password.value,
      remote_path: remotePath.value,
    });
    submitting.value = false;
    onDone(ok);
  }

  const canSubmit =
    url.value.trim().startsWith('http') && username.value && password.value;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      <Input
        label="WebDAV URL"
        value={url.value}
        onInput={(e) => (url.value = e.currentTarget.value)}
        placeholder="https://nextcloud.example.com/remote.php/dav/files/user"
        autoFocus
        required
      />
      <Input
        label="用户名"
        value={username.value}
        onInput={(e) => (username.value = e.currentTarget.value)}
        required
      />
      <Input
        label="密码"
        type="password"
        value={password.value}
        onInput={(e) => (password.value = e.currentTarget.value)}
        required
      />
      <Input
        label="远端路径"
        value={remotePath.value}
        onInput={(e) => (remotePath.value = e.currentTarget.value)}
        hint="相对 WebDAV 根目录；/ 表示根目录"
      />
      <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
        <Button variant="ghost" onClick={() => onDone(false)}>
          取消
        </Button>
        <Button
          variant="primary"
          onClick={submit}
          loading={submitting.value}
          disabled={!canSubmit}
        >
          绑定
        </Button>
      </div>
    </div>
  );
}
