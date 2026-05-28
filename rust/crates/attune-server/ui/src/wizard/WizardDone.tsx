/** Wizard · 完成页 · 2 秒自动跳转主应用 */

import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { t } from '../i18n';
import { Button } from '../components';

const TIPS_KEYS = [
  'wizard.done.tip.cmdk',
  'wizard.done.tip.switch',
  'wizard.done.tip.annotate',
] as const;

const AUTO_REDIRECT_MS = 2500;
const TIP_ROTATE_MS = 900;

export type WizardDoneProps = {
  onContinue: () => void;
};

export function WizardDone({ onContinue }: WizardDoneProps): JSX.Element {
  const [tipIdx, setTipIdx] = useState(0);

  useEffect(() => {
    // tip 轮播
    const tipTimer = setInterval(() => {
      setTipIdx((i) => (i + 1) % TIPS_KEYS.length);
    }, TIP_ROTATE_MS);

    // 2 秒后自动跳转
    const redirectTimer = setTimeout(() => {
      onContinue();
    }, AUTO_REDIRECT_MS);

    return () => {
      clearInterval(tipTimer);
      clearTimeout(redirectTimer);
    };
  }, [onContinue]);

  return (
    <div
      style={{
        minHeight: '100vh',
        background:
          'radial-gradient(ellipse at top right, #E9EEF2 0%, #F7F8FA 50%)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
      }}
    >
      <div
        className="fade-slide-in"
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          gap: 'var(--space-5)',
          textAlign: 'center',
        }}
      >
        {/* 大勾动画 */}
        <div
          aria-hidden="true"
          className="modal-in"
          style={{
            width: 96,
            height: 96,
            borderRadius: '50%',
            background: 'var(--color-accent)',
            color: 'white',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            fontSize: 48,
            boxShadow: '0 0 0 12px rgba(94, 139, 139, 0.15)',
          }}
        >
          ✓
        </div>
        <h1
          style={{
            fontSize: 'var(--text-2xl)',
            fontWeight: 700,
            color: 'var(--color-text)',
            margin: 0,
          }}
        >
          {t('wizard.done.title')}
        </h1>
        <div
          aria-live="polite"
          style={{
            minHeight: 32,
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text-secondary)',
            maxWidth: 400,
          }}
        >
          {t(TIPS_KEYS[tipIdx] ?? TIPS_KEYS[0])}
        </div>
        <Button variant="ghost" onClick={onContinue}>
          {t('wizard.done.cta')}
        </Button>
      </div>
    </div>
  );
}
