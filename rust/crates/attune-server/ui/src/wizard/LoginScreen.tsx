/** Login Screen · vault 已 setup 但 locked 时显示 · 输入 master password 解锁 */

import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import { Button, Input } from '../components';
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
      toast('success', '已解锁');
      onUnlock();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setSubmitting(false);
    }
  }

  async function handleForgotPasswordReset() {
    const first = window.confirm(
      '忘记密码后无法恢复原数据。是否清空本地知识库并重置？',
    );
    if (!first) return;

    const typed = window.prompt('请输入 RESET 确认重置：');
    if (typed !== 'RESET') {
      toast('error', '未输入 RESET，已取消重置');
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
      toast('success', '本地 Vault 已重置，请重新设置密码');
      window.location.reload();
    } catch (e) {
      setSubmitting(false);
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleResetWithRecoveryKey() {
    const recoveryKey = window.prompt('请输入恢复密钥（形如 ATN-...）：');
    if (!recoveryKey) return;
    const newPassword = window.prompt('请输入新的主密码（至少 12 位，含字母和数字）：');
    if (!newPassword) return;

    setSubmitting(true);
    setError(null);
    try {
      const res = await api.post<{ status: string; token?: string }>(
        '/vault/reset-with-recovery-key',
        { recovery_key: recoveryKey.trim(), new_password: newPassword },
        RETRY_POLICIES.destructive,
      );
      if (res.token) setToken(res.token);
      toast('success', '密码已重置并自动解锁');
      onUnlock();
    } catch (e) {
      setSubmitting(false);
      setError(e instanceof Error ? e.message : String(e));
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
            数据库已锁定 · 请输入主密码
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
            aria-label="主密码"
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
          解锁
        </Button>

        <p
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            textAlign: 'center',
            margin: 0,
          }}
        >
          忘记密码可先用恢复密钥重置并保留数据；仅在无恢复密钥时再清空重置。
        </p>

        <Button
          variant="secondary"
          size="sm"
          disabled={submitting}
          onClick={() => handleResetWithRecoveryKey()}
        >
          使用恢复密钥重置密码
        </Button>

        <Button
          variant="ghost"
          size="sm"
          disabled={submitting}
          onClick={() => handleForgotPasswordReset()}
        >
          无恢复密钥？清空并重置本地知识库
        </Button>
      </form>
    </div>
  );
}
