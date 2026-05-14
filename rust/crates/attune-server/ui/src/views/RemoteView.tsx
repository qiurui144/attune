/** Remote 视图 · Phase 6 · 本地 + WebDAV 目录管理 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState, Modal, Input } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
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
    if (!confirm(t('remote.confirm.unbind', { path: d.path }))) return;
    const ok = await unbindDir(d.id);
    if (ok) {
      toast('success', t('remote.toast.unbind_success'));
      await refresh();
    } else {
      toast('error', t('remote.toast.unbind_fail'));
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
          {`🔗 ${t('sidebar.nav.remote')}`}
        </h2>
        <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
          <Button variant="secondary" size="sm" onClick={() => (modal.value = 'local')}>
            {`📂 ${t('remote.action.add_local')}`}
          </Button>
          <Button variant="primary" size="sm" onClick={() => (modal.value = 'webdav')}>
            {`☁ ${t('remote.action.add_webdav')}`}
          </Button>
        </div>
      </header>

      {loading.value ? (
        <div style={{ color: 'var(--color-text-secondary)' }}>{t('common.loading')}</div>
      ) : dirs.value.length === 0 ? (
        <EmptyState
          icon="🔗"
          title={t('remote.empty.title')}
          description={t('remote.empty.desc')}
          actions={[
            { label: t('remote.action.add_local_folder'), onClick: () => (modal.value = 'local'), variant: 'primary' },
            { label: t('remote.action.add_webdav'), onClick: () => (modal.value = 'webdav'), variant: 'secondary' },
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
        title={t('remote.modal.local.title')}
      >
        <LocalForm
          onDone={async (result) => {
            modal.value = null;
            if (result.ok) {
              toast('success', t('remote.toast.bind_local_success'));
              await refresh();
            } else {
              toast('error', t('remote.toast.bind_local_fail', { error: result.error ?? t('remote.error.unknown') }));
            }
          }}
        />
      </Modal>

      <Modal
        open={modal.value === 'webdav'}
        onClose={() => (modal.value = null)}
        title={t('remote.modal.webdav.title')}
        maxWidth={520}
      >
        <WebdavForm
          onDone={async (result) => {
            modal.value = null;
            if (result.ok) {
              toast('success', t('remote.toast.bind_webdav_success'));
              await refresh();
            } else {
              toast('error', t('remote.toast.bind_webdav_fail', { error: result.error ?? t('remote.error.check_url_credential') }));
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
          {d.recursive ? t('remote.row.recursive') : t('remote.row.non_recursive')} · {t('remote.row.type')}: {d.file_types}
          {d.last_scan && ` · ${t('remote.row.last_scan')}: ${new Date(d.last_scan).toLocaleString()}`}
        </div>
      </div>
      <Button variant="ghost" size="sm" onClick={onUnbind}>
        {t('remote.row.unbind')}
      </Button>
    </div>
  );
}

function LocalForm({
  onDone,
}: {
  onDone: (result: { ok: boolean; error?: string }) => void;
}): JSX.Element {
  const path = useSignal('');
  const submitting = useSignal(false);

  async function submit() {
    if (!path.value.trim()) return;
    submitting.value = true;
    const result = await bindLocalDir(path.value.trim());
    submitting.value = false;
    onDone(result);
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      <Input
        label={t('remote.local.path_label')}
        value={path.value}
        onInput={(e) => (path.value = e.currentTarget.value)}
        placeholder="例：/home/qiurui/Documents/我的资料"
        autoFocus
        required
        hint={t('remote.local.path_hint')}
      />
      <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
        <Button variant="ghost" onClick={() => onDone({ ok: false })}>
          {t('common.cancel')}
        </Button>
        <Button
          variant="primary"
          onClick={submit}
          loading={submitting.value}
          disabled={!path.value.trim()}
        >
          {t('remote.local.bind')}
        </Button>
      </div>
    </div>
  );
}

function WebdavForm({
  onDone,
}: {
  onDone: (result: { ok: boolean; error?: string }) => void;
}): JSX.Element {
  const url = useSignal('');
  const username = useSignal('');
  const password = useSignal('');
  const remotePath = useSignal('/');
  const submitting = useSignal(false);

  async function submit() {
    submitting.value = true;
    const result = await bindWebdav({
      url: url.value.trim(),
      username: username.value,
      password: password.value,
      remote_path: remotePath.value,
    });
    submitting.value = false;
    onDone(result);
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
        label={t('remote.webdav.username')}
        value={username.value}
        onInput={(e) => (username.value = e.currentTarget.value)}
        required
      />
      <Input
        label={t('remote.webdav.password')}
        type="password"
        value={password.value}
        onInput={(e) => (password.value = e.currentTarget.value)}
        required
      />
      <Input
        label={t('remote.webdav.remote_path')}
        value={remotePath.value}
        onInput={(e) => (remotePath.value = e.currentTarget.value)}
        hint={t('remote.webdav.remote_path_hint')}
      />
      <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
        <Button variant="ghost" onClick={() => onDone({ ok: false })}>
          {t('common.cancel')}
        </Button>
        <Button
          variant="primary"
          onClick={submit}
          loading={submitting.value}
          disabled={!canSubmit}
        >
          {t('remote.webdav.bind')}
        </Button>
      </div>
    </div>
  );
}
