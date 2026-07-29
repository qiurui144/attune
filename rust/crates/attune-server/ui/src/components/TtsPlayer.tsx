import type { JSX } from 'preact';

import { useTts } from '../hooks/useTts';
import { t } from '../i18n';

export function TtsPlayer({ text }: { text: string }): JSX.Element {
  const tts = useTts(text);

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        flexWrap: 'wrap',
        gap: 'var(--space-2)',
        marginTop: 'var(--space-2)',
      }}
    >
      {tts.status === 'ready' && tts.audioUrl ? (
        <>
          <audio
            controls
            preload="metadata"
            src={tts.audioUrl}
            aria-label={t('chat.tts.player_label')}
            style={{ maxWidth: '100%', height: 32 }}
          />
          <button
            type="button"
            onClick={() => void tts.synthesize()}
            className="interactive"
            style={ttsButtonStyle}
          >
            {t('chat.tts.regenerate')}
          </button>
        </>
      ) : tts.status !== 'error' ? (
        <button
          type="button"
          data-testid="tts-synthesize"
          onClick={() => void tts.synthesize()}
          disabled={tts.status === 'loading'}
          className="interactive"
          style={ttsButtonStyle}
        >
          {tts.status === 'loading' ? t('chat.tts.generating') : t('chat.tts.read_aloud')}
        </button>
      ) : null}
      {tts.status === 'error' && (
        <>
          <span
            role="alert"
            title={tts.error ?? undefined}
            style={{ fontSize: 'var(--text-xs)', color: 'var(--color-error)' }}
          >
            {t('chat.tts.error')}
          </span>
          <button
            type="button"
            onClick={() => void tts.synthesize()}
            className="interactive"
            style={ttsButtonStyle}
          >
            {t('chat.tts.retry')}
          </button>
        </>
      )}
    </div>
  );
}

const ttsButtonStyle: JSX.CSSProperties = {
  padding: '3px var(--space-2)',
  background: 'var(--color-bg)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-sm)',
  color: 'var(--color-text-secondary)',
  fontSize: 'var(--text-xs)',
  cursor: 'pointer',
};
