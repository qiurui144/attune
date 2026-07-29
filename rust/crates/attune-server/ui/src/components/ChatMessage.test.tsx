import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, describe, expect, it } from 'vitest';

import { settings, type Message } from '../store/signals';
import { ChatMessage } from './ChatMessage';

function assistantMessage(content: string): Message {
  return {
    id: `message-${content.length}`,
    role: 'assistant',
    content,
    created_at: '2026-07-16T00:00:00Z',
  };
}

describe('ChatMessage TTS visibility', () => {
  afterEach(() => {
    settings.value = null;
  });

  it('keeps the TTS action for NBSP semantic text but not ASCII-space-only text', () => {
    settings.value = { tts: { enabled: true } };
    const root = document.createElement('div');

    act(() => render(<ChatMessage message={assistantMessage('\u00a0')} />, root));
    expect(root.querySelector('[data-testid="tts-synthesize"]')).not.toBeNull();

    act(() => render(null, root));
    act(() => render(<ChatMessage message={assistantMessage('   ')} />, root));
    expect(root.querySelector('[data-testid="tts-synthesize"]')).toBeNull();
    act(() => render(null, root));
  });

  it('honors the persisted TTS disabled setting', () => {
    settings.value = { tts: { enabled: false } };
    const root = document.createElement('div');
    act(() => render(<ChatMessage message={assistantMessage('readable')} />, root));
    expect(root.querySelector('[data-testid="tts-synthesize"]')).toBeNull();
    act(() => render(null, root));
  });

  it('hides TTS for text the short-speech route deterministically rejects', () => {
    settings.value = { tts: { enabled: true } };
    const root = document.createElement('div');

    for (const content of ['字'.repeat(129), 'first line\nsecond line']) {
      act(() => render(<ChatMessage message={assistantMessage(content)} />, root));
      expect(root.querySelector('[data-testid="tts-synthesize"]')).toBeNull();
      act(() => render(null, root));
    }
  });
});
