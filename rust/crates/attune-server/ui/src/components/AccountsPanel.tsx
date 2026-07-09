/** AccountsPanel · 第三方账号管理面板（弱腿增量波 C）
 *
 * 列出 / 添加 / 删除第三方源凭据。secret 输入框 type=password 不回显；列表只显示
 * 脱敏字段（provider / username / endpoint / status），永不显示 secret。
 * 所有用户可见字符串走 t()，zh/en key 集合一致（i18n 守卫）。
 */
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState, Modal, Input } from './index';
import { confirmDialog } from './ConfirmModal';
import { toast } from './Toast';
import { t } from '../i18n';
import {
  PROVIDERS,
  listThirdPartyAccounts,
  addThirdPartyAccount,
  deleteThirdPartyAccount,
} from '../hooks/useThirdPartyAccounts';
import type { Provider, ThirdPartyAccount } from '../hooks/useThirdPartyAccounts';

export function AccountsPanel(props: { onChanged?: () => void }): JSX.Element {
  const accounts = useSignal<ThirdPartyAccount[]>([]);
  const loading = useSignal(true);
  const showAdd = useSignal(false);

  // 添加表单状态。
  const fProvider = useSignal<Provider>('webdav');
  const fLabel = useSignal('');
  const fUsername = useSignal('');
  const fEndpoint = useSignal('');
  const fSecret = useSignal('');
  const submitting = useSignal(false);
  const formError = useSignal('');

  async function refresh() {
    loading.value = true;
    accounts.value = await listThirdPartyAccounts();
    loading.value = false;
  }

  useEffect(() => {
    void refresh();
  }, []);

  function resetForm() {
    fProvider.value = 'webdav';
    fLabel.value = '';
    fUsername.value = '';
    fEndpoint.value = '';
    fSecret.value = ''; // 关键：清空 secret，不缓存。
    formError.value = '';
  }

  async function handleAdd() {
    if (!fEndpoint.value.trim() || !fSecret.value) {
      formError.value = t('accounts.error.required');
      return;
    }
    submitting.value = true;
    const res = await addThirdPartyAccount({
      provider: fProvider.value,
      label: fLabel.value,
      username: fUsername.value,
      endpoint: fEndpoint.value,
      secret: fSecret.value,
    });
    submitting.value = false;
    if (res.ok) {
      toast('success', t('accounts.toast.added'));
      showAdd.value = false;
      resetForm();
      await refresh();
      props.onChanged?.();
    } else {
      formError.value = res.error || t('accounts.error.addFail');
    }
  }

  async function handleDelete(acc: ThirdPartyAccount) {
    const okToDelete = await confirmDialog({
      title: t('confirm.title.deleteThirdPartyAccount'),
      message: t('accounts.confirm.delete', { label: acc.label || acc.username || acc.id }),
      danger: true,
    });
    if (!okToDelete) {
      return;
    }
    const ok = await deleteThirdPartyAccount(acc.id);
    if (ok) {
      toast('success', t('accounts.toast.deleted'));
      await refresh();
      props.onChanged?.();
    } else {
      toast('error', t('accounts.toast.deleteFail'));
    }
  }

  return (
    <section style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
      <header style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <h3 style={{ fontSize: 'var(--text-lg)', fontWeight: 600, margin: 0 }}>
          {t('accounts.title')}
        </h3>
        <Button variant="primary" onClick={() => { resetForm(); showAdd.value = true; }}>
          {t('accounts.add')}
        </Button>
      </header>

      {loading.value ? (
        <p style={{ color: 'var(--text-muted)' }}>{t('common.loading')}</p>
      ) : accounts.value.length === 0 ? (
        <EmptyState title={t('accounts.empty.title')} description={t('accounts.empty.body')} />
      ) : (
        <table style={{ width: '100%', borderCollapse: 'collapse' }}>
          <thead>
            <tr style={{ textAlign: 'left', color: 'var(--text-muted)' }}>
              <th style={{ padding: 'var(--space-2)' }}>{t('accounts.col.provider')}</th>
              <th style={{ padding: 'var(--space-2)' }}>{t('accounts.col.username')}</th>
              <th style={{ padding: 'var(--space-2)' }}>{t('accounts.col.endpoint')}</th>
              <th style={{ padding: 'var(--space-2)' }}>{t('accounts.col.status')}</th>
              <th style={{ padding: 'var(--space-2)' }}>{t('accounts.col.actions')}</th>
            </tr>
          </thead>
          <tbody>
            {accounts.value.map((acc) => (
              <tr key={acc.id} style={{ borderTop: '1px solid var(--border)' }}>
                <td style={{ padding: 'var(--space-2)' }}>{acc.provider}</td>
                <td style={{ padding: 'var(--space-2)' }}>{acc.username || '—'}</td>
                <td style={{ padding: 'var(--space-2)' }}>{acc.endpoint}</td>
                <td style={{ padding: 'var(--space-2)' }}>{acc.status}</td>
                <td style={{ padding: 'var(--space-2)' }}>
                  <Button variant="ghost" size="sm" onClick={() => void handleDelete(acc)}>
                    {t('accounts.delete')}
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <Modal open={showAdd.value} onClose={() => { showAdd.value = false; }} title={t('accounts.add')}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
          <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-1)' }}>
            <span style={{ fontSize: 'var(--text-sm)' }}>{t('accounts.col.provider')}</span>
            <select
              value={fProvider.value}
              onChange={(e) => { fProvider.value = (e.currentTarget as HTMLSelectElement).value as Provider; }}
              aria-label={t('accounts.col.provider')}
            >
              {PROVIDERS.map((p) => (
                <option key={p} value={p}>{p}</option>
              ))}
            </select>
          </label>
          <Input
            label={t('accounts.field.label')}
            value={fLabel.value}
            placeholder={t('accounts.field.labelPlaceholder')}
            onInput={(e) => { fLabel.value = e.currentTarget.value; }}
          />
          <Input
            label={t('accounts.field.username')}
            value={fUsername.value}
            onInput={(e) => { fUsername.value = e.currentTarget.value; }}
          />
          <Input
            label={t('accounts.field.endpoint')}
            type="url"
            required
            value={fEndpoint.value}
            placeholder={t('accounts.field.endpointPlaceholder')}
            onInput={(e) => { fEndpoint.value = e.currentTarget.value; }}
          />
          <Input
            label={t('accounts.field.secret')}
            type="password"
            required
            value={fSecret.value}
            hint={t('accounts.field.secretHint')}
            onInput={(e) => { fSecret.value = e.currentTarget.value; }}
          />
          {formError.value && (
            <p style={{ color: 'var(--danger)', fontSize: 'var(--text-sm)' }}>{formError.value}</p>
          )}
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
            <Button variant="ghost" onClick={() => { showAdd.value = false; }}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" disabled={submitting.value} onClick={() => void handleAdd()}>
              {submitting.value ? t('common.saving') : t('accounts.connect')}
            </Button>
          </div>
        </div>
      </Modal>
    </section>
  );
}
