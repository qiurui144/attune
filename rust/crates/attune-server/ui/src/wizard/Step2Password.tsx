/** Wizard Step 2 · Master Password（唯一硬门槛） */

import type { JSX } from 'preact';
import { useState, useMemo } from 'preact/hooks';
import { Button, Input } from '../components';
import { t } from '../i18n';
import { api, setToken } from '../store/api';
import { toast } from '../components/Toast';

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
  onContinue: () => void;
};

export function Step2Password({ onContinue }: Step2Props): JSX.Element {
  const [pwd, setPwd] = useState('');
  const [confirm, setConfirm] = useState('');
  const [show, setShow] = useState(false);
  const [exportSecret, setExportSecret] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const strength = evalStrength(pwd);

  const tooShort = pwd.length > 0 && pwd.length < 12;
  const tooWeak =
    pwd.length >= 12 && (!/[a-zA-Z]/.test(pwd) || !/\d/.test(pwd));
  const mismatch = confirm.length > 0 && pwd !== confirm;
  const canSubmit =
    pwd.length >= 12 &&
    /[a-zA-Z]/.test(pwd) &&
    /\d/.test(pwd) &&
    pwd === confirm &&
    !submitting;

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
      const res = await api.post<{ status: string; state?: string; token?: string }>(
        '/vault/setup',
        { password: pwd },
      );
      if (res.token) setToken(res.token);

      // 可选：生成 device secret 文件（后端有端点 export_device_secret）
      if (exportSecret) {
        try {
          const secretRes = await api.get<{ device_secret: string }>(
            '/vault/device-secret/export',
          );
          downloadText('attune-device-secret.txt', secretRes.device_secret);
          toast('info', '已下载 Device Secret（妥善保管，可用于其他设备导入）');
        } catch {
          toast('warning', 'Device Secret 导出失败，可以稍后在 Settings 里再生成');
        }
      }

      onContinue();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setSubmitting(false);
    }
  }

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
        <div>
          <Input
            label={t('wizard.pwd.field')}
            type={show ? 'text' : 'password'}
            value={pwd}
            onInput={(e) => setPwd(e.currentTarget.value)}
            error={pwdError}
            autoFocus
            required
          />
          {strength && (
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

        <Input
          label={t('wizard.pwd.confirm')}
          type={show ? 'text' : 'password'}
          value={confirm}
          onInput={(e) => setConfirm(e.currentTarget.value)}
          error={confirmError}
          required
        />

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

      <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
        <Button
          variant="primary"
          size="lg"
          disabled={!canSubmit}
          loading={submitting}
          onClick={handleSubmit}
        >
          {t('common.next')} →
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
