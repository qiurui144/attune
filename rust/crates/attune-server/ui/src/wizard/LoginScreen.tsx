/** Login Screen · vault 已 setup 但 locked 时显示 · 输入 master password 解锁 */

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { Button, Input, Modal } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import { api, clearToken, setToken, RETRY_POLICIES } from '../store/api';

export type LoginScreenProps = {
  onUnlock: () => void;
};

export function LoginScreen({ onUnlock }: LoginScreenProps): JSX.Element {
  const [pwd, setPwd] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showRecoveryModal, setShowRecoveryModal] = useState(false);
  const [recoveryKey, setRecoveryKey] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [recoveryError, setRecoveryError] = useState<string | null>(null);

  async function handleUnlock(e?: Event) {
    e?.preventDefault();
    if (!pwd) return;
    setSubmitting(true);
    setError(null);
    try {
      // Important 2.3：密码验证不走自动重试（防失败锁账号）
      const res = await api.post<{ status: string; token?: string }>(
        '/vault/unlock',
        { password: pwd },
        RETRY_POLICIES.destructive,
      );
      if (res.token) setToken(res.token);
      toast('success', t('lock.toast.unlocked'));
      onUnlock();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setSubmitting(false);
    }
  }

  async function handleForgotPasswordReset() {
    const first = window.confirm(t('lock.confirm.wipe'));
    if (!first) return;

    const typed = window.prompt(t('lock.prompt.reset_confirm'));
    if (typed !== 'RESET') {
      toast('error', t('lock.toast.reset_cancelled'));
      return;
    }

    setSubmitting(true);
    setError(null);
    try {
      await api.post<{ status: string }>(
        '/vault/forgot-password-reset',
        { confirmation: 'RESET' },
        RETRY_POLICIES.destructive,
      );
      clearToken();
      toast('success', t('lock.toast.vault_reset'));
      window.location.reload();
    } catch (e) {
      setSubmitting(false);
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  function openRecoveryModal() {
    setRecoveryKey('');
    setNewPassword('');
    setRecoveryError(null);
    setShowRecoveryModal(true);
  }

  async function handleResetWithRecoveryKey() {
    const key = recoveryKey.trim();
    if (!key || !newPassword) return;

    setSubmitting(true);
    setRecoveryError(null);
    try {
      const res = await api.post<{ status: string; token?: string }>(
        '/vault/reset-with-recovery-key',
        { recovery_key: key, new_password: newPassword },
        RETRY_POLICIES.destructive,
      );
      if (res.token) setToken(res.token);
      setSubmitting(false);
      setShowRecoveryModal(false);
      toast('success', t('lock.toast.password_reset'));
      onUnlock();
    } catch (e) {
      setSubmitting(false);
      setRecoveryError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div
      style={{
        minHeight: '100vh',
        background:
          'radial-gradient(ellipse at top right, #E9EEF2 0%, #F7F8FA 50%)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 'var(--space-5)',
      }}
    >
      <form
        onSubmit={handleUnlock}
        className="fade-slide-in"
        style={{
          background: 'var(--color-surface)',
          borderRadius: 'var(--radius-xl)',
          boxShadow: 'var(--shadow-lg)',
          padding: 'var(--space-7) var(--space-6)',
          maxWidth: 400,
          width: '100%',
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-5)',
          alignItems: 'center',
        }}
      >
        <div style={{ fontSize: 48 }} aria-hidden="true">
          🔒
        </div>
        <div style={{ textAlign: 'center' }}>
          <h1
            style={{
              fontSize: 'var(--text-xl)',
              fontWeight: 600,
              margin: 0,
              marginBottom: 'var(--space-2)',
            }}
          >
            {t('app.name')}
          </h1>
          <p
            style={{
              fontSize: 'var(--text-sm)',
              color: 'var(--color-text-secondary)',
              margin: 0,
            }}
          >
            {t('lock.subtitle')}
          </p>
        </div>

        <div style={{ width: '100%' }}>
          <Input
            type="password"
            value={pwd}
            onInput={(e) => setPwd(e.currentTarget.value)}
            error={error ?? undefined}
            autoFocus
            required
            aria-label={t('lock.password_label')}
            placeholder="••••••••••••"
          />
        </div>

        <Button
          type="submit"
          variant="primary"
          size="lg"
          fullWidth
          loading={submitting}
          disabled={!pwd}
          onClick={() => handleUnlock()}
        >
          {t('lock.unlock')}
        </Button>

        <p
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            textAlign: 'center',
            margin: 0,
          }}
        >
          {t('lock.recovery_hint')}
        </p>

        <Button
          variant="secondary"
          size="sm"
          disabled={submitting}
          onClick={openRecoveryModal}
        >
          {t('lock.reset_with_recovery')}
        </Button>

        <Button
          variant="ghost"
          size="sm"
          disabled={submitting}
          onClick={() => handleForgotPasswordReset()}
        >
          {t('lock.reset_wipe')}
        </Button>
      </form>

      <Modal
        open={showRecoveryModal}
        onClose={() => setShowRecoveryModal(false)}
        title={t('lock.recovery_modal.title')}
      >
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
          <Input
            label={t('lock.recovery_modal.key_label')}
            value={recoveryKey}
            onInput={(e) => setRecoveryKey(e.currentTarget.value)}
            placeholder="ATN-..."
            autoFocus
            required
          />
          <Input
            type="password"
            label={t('lock.recovery_modal.new_password_label')}
            value={newPassword}
            onInput={(e) => setNewPassword(e.currentTarget.value)}
            hint={t('lock.recovery_modal.new_password_hint')}
            error={recoveryError ?? undefined}
            required
          />
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 'var(--space-2)' }}>
            <Button variant="ghost" onClick={() => setShowRecoveryModal(false)}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              loading={submitting}
              disabled={!recoveryKey.trim() || !newPassword}
              onClick={() => void handleResetWithRecoveryKey()}
            >
              {t('lock.recovery_modal.submit')}
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
}
