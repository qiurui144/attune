/** Wizard Step 2 · Master Password（唯一硬门槛） */

import type { JSX } from 'preact';
import { useState, useMemo, useEffect } from 'preact/hooks';
import { Button, Input } from '../components';
import { t } from '../i18n';
import { api, setToken, RETRY_POLICIES } from '../store/api';
import { toast } from '../components/Toast';
import type { WizardContext } from './types';

type Strength = 'weak' | 'medium' | 'strong';

function evalStrength(pwd: string): Strength | null {
  if (pwd.length < 12) return null;
  const hasLetter = /[a-zA-Z]/.test(pwd);
  const hasDigit = /\d/.test(pwd);
  const hasSpecial = /[^a-zA-Z0-9]/.test(pwd);
  const long = pwd.length >= 16;
  const score = [hasLetter, hasDigit, hasSpecial, long].filter(Boolean).length;
  if (score >= 4) return 'strong';
  if (score >= 3) return 'medium';
  return 'weak';
}

const STRENGTH_COLORS: Record<Strength, string> = {
  weak: 'var(--color-error)',
  medium: 'var(--color-warning)',
  strong: 'var(--color-success)',
};

export type Step2Props = {
  ctx: WizardContext;
  onUpdate: (partial: Partial<WizardContext>) => void;
  onContinue: () => void;
};

export function Step2Password({ ctx, onUpdate, onContinue }: Step2Props): JSX.Element {
  const [pwd, setPwd] = useState('');
  const [confirm, setConfirm] = useState('');
  const [show, setShow] = useState(false);
  const [exportSecret, setExportSecret] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  // submitStage 让按钮文案 / aria-live 提示能反映"正在派生主密钥"vs"正在解锁"vs"提交中".
  // Argon2id 派生在 setup 路径上耗时 ~10s, 静默会被误认为"卡住".
  const [submitStage, setSubmitStage] = useState<'idle' | 'deriving' | 'unlocking'>('idle');
  const [error, setError] = useState<string | null>(null);
  const [vaultState, setVaultState] = useState<'checking' | 'sealed' | 'locked' | 'unlocked'>('checking');
  const [memberEmail, setMemberEmail] = useState(ctx.memberEmail ?? '');
  const [memberPassword, setMemberPassword] = useState(ctx.memberPassword ?? '');
  const [memberLicenseCode, setMemberLicenseCode] = useState(ctx.memberLicenseCode ?? '');

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const status = await api.get<{ state?: 'sealed' | 'locked' | 'unlocked' }>('/vault/status');
        if (!active) return;
        setVaultState(status.state ?? 'sealed');
      } catch {
        if (!active) return;
        setVaultState('sealed');
      }
    })();
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    onUpdate({
      memberEmail: memberEmail.trim() || null,
      memberPassword: memberPassword || null,
      memberLicenseCode: memberLicenseCode.trim() || null,
    });
  }, [memberEmail, memberPassword, memberLicenseCode, onUpdate]);

  const strength = evalStrength(pwd);

  const tooShort = pwd.length > 0 && pwd.length < 12;
  const tooWeak =
    pwd.length >= 12 && (!/[a-zA-Z]/.test(pwd) || !/\d/.test(pwd));
  const mismatch = confirm.length > 0 && pwd !== confirm;
  const isSetupMode = vaultState === 'sealed';
  const canSubmit = isSetupMode
    ? (
      pwd.length >= 12 &&
      /[a-zA-Z]/.test(pwd) &&
      /\d/.test(pwd) &&
      pwd === confirm &&
      !submitting
    )
    : (!submitting && (vaultState === 'unlocked' || pwd.length > 0));

  const pwdError = useMemo(() => {
    if (tooShort) return t('wizard.pwd.err.too_short');
    if (tooWeak) return t('wizard.pwd.err.too_weak');
    return undefined;
  }, [tooShort, tooWeak]);

  const confirmError = mismatch ? t('wizard.pwd.err.mismatch') : undefined;

  async function handleSubmit() {
    if (!canSubmit) return;
    setSubmitting(true);
    setError(null);
    try {
      if (isSetupMode) {
        // Argon2id 派生 ~10s, 提前给用户"正在派生主密钥"的视觉信号 + toast.
        setSubmitStage('deriving');
        toast('info', t('wizard.pwd.toast_deriving'));
        // Important 2.3：密码 setup 不走自动重试
        const res = await api.post<{ status: string; state?: string; token?: string; recovery_key?: string }>(
          '/vault/setup',
          { password: pwd },
          RETRY_POLICIES.destructive,
        );
        if (res.token) setToken(res.token);
        if (res.recovery_key) {
          downloadText('attune-recovery-key.txt', res.recovery_key);
          toast('warning', t('wizard.pwd.recovery_downloaded'));
        }

        // 可选：生成 device secret 文件（后端有端点 export_device_secret）
        if (exportSecret) {
          try {
            const secretRes = await api.get<{ device_secret: string }>(
              '/vault/device-secret/export',
            );
            downloadText('attune-device-secret.txt', secretRes.device_secret);
            toast('info', t('wizard.pwd.device_secret_downloaded'));
          } catch {
            toast('warning', t('wizard.pwd.device_secret_failed'));
          }
        }
      } else if (vaultState === 'locked') {
        setSubmitStage('unlocking');
        const unlock = await api.post<{ token?: string }>('/vault/unlock', { password: pwd });
        if (unlock.token) setToken(unlock.token);
      }

      onContinue();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);

      // setup 和状态刷新可能存在竞争：后端已初始化时自动回退到 unlock 流程，避免卡死。
      if (isSetupMode && msg.includes('already initialized') && pwd.length > 0) {
        try {
          const unlock = await api.post<{ token?: string }>('/vault/unlock', { password: pwd });
          if (unlock.token) setToken(unlock.token);
          onContinue();
          return;
        } catch {
          // fall through to show original error
        }
      }

      setError(msg);
      setSubmitting(false);
      setSubmitStage('idle');
    }
  }

  // 按钮文案: idle 时显示 "Next →", 派生/解锁中显示对应进度文案.
  const submitLabel =
    submitStage === 'deriving' ? t('wizard.pwd.btn_deriving') :
    submitStage === 'unlocking' ? t('wizard.pwd.btn_unlocking') :
    `${t('common.next')} →`;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-5)' }}>
      <header>
        <h2
          style={{
            fontSize: 'var(--text-xl)',
            fontWeight: 600,
            margin: 0,
            marginBottom: 'var(--space-2)',
          }}
        >
          {t('wizard.pwd.heading')}
        </h2>
        <div
          role="alert"
          style={{
            padding: 'var(--space-3)',
            background: 'rgba(212, 165, 116, 0.1)',
            border: '1px solid var(--color-warning)',
            borderRadius: 'var(--radius-md)',
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text)',
          }}
        >
          {t('wizard.pwd.warning')}
        </div>
      </header>

      <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-3)' }}>
        {!isSetupMode && (
          <div
            style={{
              padding: 'var(--space-2) var(--space-3)',
              background: 'var(--color-surface)',
              border: '1px solid var(--color-border)',
              borderRadius: 'var(--radius-sm)',
              fontSize: 'var(--text-sm)',
              color: 'var(--color-text-secondary)',
            }}
          >
            {vaultState === 'unlocked'
              ? t('wizard.pwd.unlocked_hint')
              : t('wizard.pwd.locked_hint')}
          </div>
        )}
        <div>
          <Input
            label={t('wizard.pwd.field')}
            type={show ? 'text' : 'password'}
            value={pwd}
            onInput={(e) => setPwd(e.currentTarget.value)}
            error={isSetupMode ? pwdError : undefined}
            autoFocus
            required={vaultState !== 'unlocked'}
          />
          {isSetupMode && strength && (
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 'var(--space-2)',
                marginTop: 'var(--space-1)',
                fontSize: 'var(--text-xs)',
                color: 'var(--color-text-secondary)',
              }}
            >
              <div
                style={{
                  flex: 1,
                  height: 4,
                  background: 'var(--color-border)',
                  borderRadius: 2,
                  overflow: 'hidden',
                }}
              >
                <div
                  style={{
                    height: '100%',
                    width:
                      strength === 'weak'
                        ? '33%'
                        : strength === 'medium'
                          ? '66%'
                          : '100%',
                    background: STRENGTH_COLORS[strength],
                    transition: 'all var(--duration-base) var(--ease-out)',
                  }}
                />
              </div>
              <span style={{ color: STRENGTH_COLORS[strength] }}>
                {t(`wizard.pwd.strength.${strength}`)}
              </span>
            </div>
          )}
        </div>

        {isSetupMode && (
          <Input
            label={t('wizard.pwd.confirm')}
            type={show ? 'text' : 'password'}
            value={confirm}
            onInput={(e) => setConfirm(e.currentTarget.value)}
            error={confirmError}
            required
          />
        )}

        <label
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 'var(--space-2)',
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text-secondary)',
            cursor: 'pointer',
          }}
        >
          <input
            type="checkbox"
            checked={show}
            onChange={(e) => setShow(e.currentTarget.checked)}
          />
          {show ? t('wizard.pwd.hide') : t('wizard.pwd.show')}
        </label>

        {isSetupMode && (
          <label
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 'var(--space-2)',
              fontSize: 'var(--text-sm)',
              color: 'var(--color-text-secondary)',
              cursor: 'pointer',
            }}
          >
            <input
              type="checkbox"
              checked={exportSecret}
              onChange={(e) => setExportSecret(e.currentTarget.checked)}
            />
            {t('wizard.pwd.export_secret')}
          </label>
        )}
      </div>

      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-2)',
          marginTop: 'var(--space-2)',
          padding: 'var(--space-3)',
          border: '1px solid var(--color-border)',
          borderRadius: 'var(--radius-md)',
          background: 'var(--color-bg)',
        }}
      >
        <div style={{ fontWeight: 600, fontSize: 'var(--text-sm)' }}>{t('wizard.member.heading')}</div>
        <div style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-xs)' }}>
          {t('wizard.member.desc')}
        </div>
        <Input
          type="text"
          label={t('wizard.member.email')}
          value={memberEmail}
          onInput={(e) => setMemberEmail(e.currentTarget.value)}
          placeholder={t('wizard.member.email_placeholder')}
        />
        <Input
          type="password"
          label={t('wizard.member.password')}
          value={memberPassword}
          onInput={(e) => setMemberPassword(e.currentTarget.value)}
          placeholder={t('wizard.member.password_placeholder')}
        />
        <Input
          type="text"
          label={t('wizard.member.license_code')}
          value={memberLicenseCode}
          onInput={(e) => setMemberLicenseCode(e.currentTarget.value)}
          placeholder={t('wizard.member.license_code_placeholder')}
        />
      </div>

      {error && (
        <div
          role="alert"
          style={{
            padding: 'var(--space-3)',
            background: 'rgba(201, 112, 112, 0.1)',
            border: '1px solid var(--color-error)',
            borderRadius: 'var(--radius-md)',
            fontSize: 'var(--text-sm)',
            color: 'var(--color-error)',
          }}
        >
          {error}
        </div>
      )}

      {/* 派生过程 aria-live 通知, 屏幕阅读器 + 视觉双通道 */}
      {submitStage !== 'idle' && (
        <div
          role="status"
          aria-live="polite"
          style={{
            padding: 'var(--space-2) var(--space-3)',
            background: 'var(--color-surface)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-sm)',
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text-secondary)',
          }}
        >
          ⏳ {submitLabel}
        </div>
      )}

      <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
        <Button
          variant="primary"
          size="lg"
          disabled={!canSubmit}
          loading={submitting}
          onClick={handleSubmit}
        >
          {submitLabel}
        </Button>
      </div>
    </div>
  );
}

function downloadText(filename: string, text: string): void {
  const blob = new Blob([text], { type: 'text/plain' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}
