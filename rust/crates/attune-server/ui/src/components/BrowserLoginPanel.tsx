/** BrowserLoginPanel · 浏览器登入协助会话面板（GAP-A）
 *
 * 列已连接登入源（脱敏：源名 + 域名 + 状态，**绝不显示凭据 / 会话**）+ "连接新源"
 * 按钮（选 scan/login 模式 → 触发 → 人在回路提示 → 成功落保险柜）+ 移除。
 *
 * 成本契约：login 走人在回路（本地浏览器，🆓 本地），auto 模式默认**关**且需付费
 * 会员（💰 cloud LLM）—— 默认 toggle 关 + consent 文案，与全局成本契约一致。
 * 所有用户可见字符串走 t()，zh/en key 集合一致（i18n 双守卫）。
 */
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState, Modal, Input } from './index';
import { toast } from './Toast';
import { t } from '../i18n';
import {
  listBrowserLoginSessions,
  connectBrowserLogin,
  deleteBrowserLoginSession,
} from '../hooks/useBrowserLogin';
import type { BrowserLoginSession, ConnectMode } from '../hooks/useBrowserLogin';

export function BrowserLoginPanel(): JSX.Element {
  const sessions = useSignal<BrowserLoginSession[]>([]);
  const loading = useSignal(true);
  const showConnect = useSignal(false);

  // 连接表单状态。
  const fSourceName = useSignal('');
  const fEntryUrl = useSignal('');
  const fMode = useSignal<ConnectMode>('login');
  const fAuto = useSignal(false); // auto 默认关（L-5）。
  const submitting = useSignal(false);
  const formError = useSignal('');

  async function refresh() {
    loading.value = true;
    sessions.value = await listBrowserLoginSessions();
    loading.value = false;
  }

  useEffect(() => {
    void refresh();
  }, []);

  function resetForm() {
    fSourceName.value = '';
    fEntryUrl.value = '';
    fMode.value = 'login';
    fAuto.value = false;
    formError.value = '';
  }

  async function handleConnect() {
    if (!fEntryUrl.value.trim()) {
      formError.value = t('browserLogin.error.urlRequired');
      return;
    }
    submitting.value = true;
    formError.value = '';
    const res = await connectBrowserLogin({
      source_name: fSourceName.value,
      entry_url: fEntryUrl.value,
      mode: fMode.value,
      auto: fAuto.value,
    });
    submitting.value = false;
    if (res.ok) {
      if (res.captured) {
        toast('success', t('browserLogin.toast.captured'));
        showConnect.value = false;
        resetForm();
        await refresh();
      } else if (res.needs_login) {
        // scan 探到需要登录 —— 提示用户改用 login 模式完成人在回路。
        toast('info', t('browserLogin.toast.needsLogin'));
      } else {
        toast('info', t('browserLogin.toast.outcome', { outcome: res.outcome || '' }));
      }
    } else {
      formError.value = res.error || t('browserLogin.error.connectFail');
    }
  }

  async function handleDelete(s: BrowserLoginSession) {
    if (!confirm(t('browserLogin.confirm.delete', { name: s.source_name || s.domain || s.id }))) {
      return;
    }
    const ok = await deleteBrowserLoginSession(s.id);
    if (ok) {
      toast('success', t('browserLogin.toast.deleted'));
      await refresh();
    } else {
      toast('error', t('browserLogin.toast.deleteFail'));
    }
  }

  return (
    <section style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      <header style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <h3 style={{ fontSize: 'var(--text-lg)', fontWeight: 600, margin: 0 }}>
          {t('browserLogin.title')}
        </h3>
        <Button variant="primary" onClick={() => { resetForm(); showConnect.value = true; }}>
          {t('browserLogin.connect')}
        </Button>
      </header>

      <p style={{ color: 'var(--text-muted)', fontSize: 'var(--text-sm)', margin: 0 }}>
        {t('browserLogin.subtitle')}
      </p>

      {loading.value ? (
        <p style={{ color: 'var(--text-muted)' }}>{t('common.loading')}</p>
      ) : sessions.value.length === 0 ? (
        <EmptyState title={t('browserLogin.empty.title')} description={t('browserLogin.empty.body')} />
      ) : (
        <table style={{ width: '100%', borderCollapse: 'collapse' }}>
          <thead>
            <tr style={{ textAlign: 'left', color: 'var(--text-muted)' }}>
              <th style={{ padding: 'var(--space-2)' }}>{t('browserLogin.col.source')}</th>
              <th style={{ padding: 'var(--space-2)' }}>{t('browserLogin.col.domain')}</th>
              <th style={{ padding: 'var(--space-2)' }}>{t('browserLogin.col.status')}</th>
              <th style={{ padding: 'var(--space-2)' }}>{t('browserLogin.col.actions')}</th>
            </tr>
          </thead>
          <tbody>
            {sessions.value.map((s) => (
              <tr key={s.id} style={{ borderTop: '1px solid var(--border)' }}>
                <td style={{ padding: 'var(--space-2)' }}>{s.source_name || '—'}</td>
                <td style={{ padding: 'var(--space-2)' }}>{s.domain}</td>
                <td style={{ padding: 'var(--space-2)' }}>{s.status}</td>
                <td style={{ padding: 'var(--space-2)' }}>
                  <Button variant="ghost" size="sm" onClick={() => void handleDelete(s)}>
                    {t('browserLogin.action.remove')}
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {showConnect.value && (
        <Modal
          open
          title={t('browserLogin.modal.title')}
          onClose={() => { showConnect.value = false; }}
        >
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-1)' }}>
              <span>{t('browserLogin.field.sourceName')}</span>
              <Input
                value={fSourceName.value}
                onInput={(e) => (fSourceName.value = (e.target as HTMLInputElement).value)}
                placeholder={t('browserLogin.field.sourceNamePlaceholder')}
              />
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-1)' }}>
              <span>{t('browserLogin.field.entryUrl')}</span>
              <Input
                value={fEntryUrl.value}
                onInput={(e) => (fEntryUrl.value = (e.target as HTMLInputElement).value)}
                placeholder={t('browserLogin.field.entryUrlPlaceholder')}
              />
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-1)' }}>
              <span>{t('browserLogin.field.mode')}</span>
              <select
                value={fMode.value}
                onChange={(e) => (fMode.value = (e.target as HTMLSelectElement).value as ConnectMode)}
                style={{ padding: 'var(--space-2)' }}
              >
                <option value="login">{t('browserLogin.mode.login')}</option>
                <option value="scan">{t('browserLogin.mode.scan')}</option>
              </select>
            </label>

            {/* auto 模式：默认关 + consent 文案（💰 付费会员 cloud LLM）。 */}
            <label style={{ display: 'flex', alignItems: 'flex-start', gap: 'var(--space-2)' }}>
              <input
                type="checkbox"
                checked={fAuto.value}
                onChange={(e) => (fAuto.value = (e.target as HTMLInputElement).checked)}
              />
              <span style={{ fontSize: 'var(--text-sm)', color: 'var(--text-muted)' }}>
                {t('browserLogin.field.autoConsent')}
              </span>
            </label>

            <p style={{ fontSize: 'var(--text-sm)', color: 'var(--text-muted)', margin: 0 }}>
              {fMode.value === 'login' ? t('browserLogin.hint.humanInLoop') : t('browserLogin.hint.scan')}
            </p>

            {formError.value && (
              <p style={{ color: 'var(--danger)', fontSize: 'var(--text-sm)', margin: 0 }}>
                {formError.value}
              </p>
            )}

            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
              <Button variant="secondary" onClick={() => { showConnect.value = false; }}>
                {t('common.cancel')}
              </Button>
              <Button variant="primary" disabled={submitting.value} onClick={() => void handleConnect()}>
                {submitting.value ? t('browserLogin.action.connecting') : t('browserLogin.action.start')}
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </section>
  );
}
