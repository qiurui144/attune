/**
 * PrivacyTour · v1.0.6 one-shot modal shown the first time the app boots.
 *
 * Mounts inside the main App phase. Reads `privacy_tour_seen` from
 * `/api/v1/privacy/status` and only opens if it is `false` (default).
 * On dismiss it PATCHes `/privacy/settings` { privacy_tour_seen: true }
 * so the modal never reappears.
 *
 * 见 spec `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md` §5.1
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { api, ApiError } from '../store/api';
import { vaultState, currentView } from '../store/signals';
import { t } from '../i18n';

interface PrivacyStatus {
  outbound: Record<string, { enabled: boolean }>;
  vault: { state: string };
  redactor: { patterns_active: number };
  privacy_tour_seen?: boolean;
}

export function PrivacyTour(): JSX.Element | null {
  const open = useSignal(false);

  useEffect(() => {
    // Only check after vault is unlocked — otherwise the endpoint 401s and
    // we'd flash the modal at the wrong moment.
    if (vaultState.value !== 'unlocked') return;

    let cancelled = false;
    void api
      .get<PrivacyStatus>('/privacy/status')
      .then((s) => {
        if (cancelled) return;
        if (s.privacy_tour_seen !== true) open.value = true;
      })
      .catch((e) => {
        // 401 / network errors silently skip — user will see the tour next
        // time the page boots while unlocked.
        if (e instanceof ApiError && e.status === 401) return;
      });

    return () => {
      cancelled = true;
    };
  }, [vaultState.value]);

  async function dismiss(): Promise<void> {
    open.value = false;
    try {
      await api.patch('/privacy/settings', { privacy_tour_seen: true });
    } catch {
      // best-effort — if the patch fails the modal will reappear next session
      // (acceptable; better than blocking the user).
    }
  }

  function openPrivacy(): void {
    void dismiss();
    currentView.value = 'privacy';
  }

  if (!open.value) return null;

  return (
    <div
      data-testid="privacy-tour-modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="privacy-tour-title"
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0, 0, 0, 0.5)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        zIndex: 1000,
        padding: 'var(--space-4)',
      }}
    >
      <div
        style={{
          background: 'var(--color-surface)',
          color: 'var(--color-text)',
          borderRadius: 'var(--radius-lg)',
          padding: 'var(--space-5)',
          maxWidth: 480,
          width: '100%',
          boxShadow: '0 10px 30px rgba(0, 0, 0, 0.25)',
        }}
      >
        <h2
          id="privacy-tour-title"
          style={{
            margin: 0,
            marginBottom: 'var(--space-2)',
            fontSize: 'var(--text-xl)',
            fontWeight: 600,
          }}
        >
          {t('privacy.tour.title')}
        </h2>
        <p
          style={{
            margin: 0,
            marginBottom: 'var(--space-4)',
            fontSize: 'var(--text-sm)',
            color: 'var(--color-text-secondary)',
            lineHeight: 1.6,
          }}
        >
          {t('privacy.tour.intro')}
        </p>
        <ul
          style={{
            margin: 0,
            marginBottom: 'var(--space-4)',
            paddingLeft: 'var(--space-4)',
            fontSize: 'var(--text-sm)',
            lineHeight: 1.8,
          }}
        >
          <li>{t('privacy.outbound.llm')}</li>
          <li>{t('privacy.outbound.cloudSaas')}</li>
          <li>{t('privacy.outbound.webdav')}</li>
          <li>{t('privacy.outbound.webSearch')}</li>
          <li>{t('privacy.outbound.telemetry')}</li>
        </ul>
        <div
          style={{
            display: 'flex',
            justifyContent: 'flex-end',
            gap: 'var(--space-2)',
          }}
        >
          <button
            type="button"
            onClick={openPrivacy}
            className="interactive"
            style={{
              padding: 'var(--space-2) var(--space-4)',
              background: 'transparent',
              color: 'var(--color-text)',
              border: '1px solid var(--color-border)',
              borderRadius: 'var(--radius-md)',
              fontSize: 'var(--text-sm)',
              cursor: 'pointer',
            }}
            data-testid="privacy-tour-open-dashboard"
          >
            {t('privacy.title')}
          </button>
          <button
            type="button"
            onClick={() => void dismiss()}
            className="interactive"
            style={{
              padding: 'var(--space-2) var(--space-4)',
              background: 'var(--color-accent)',
              color: 'white',
              border: 'none',
              borderRadius: 'var(--radius-md)',
              fontSize: 'var(--text-sm)',
              cursor: 'pointer',
            }}
            data-testid="privacy-tour-dismiss"
          >
            {t('privacy.tour.cta')}
          </button>
        </div>
      </div>
    </div>
  );
}
