import { render } from 'preact';
import { act } from 'preact/test-utils';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { setLocale } from '../i18n';
import { TtsPlayer } from './TtsPlayer';

describe('TtsPlayer', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    sessionStorage.clear();
    setLocale('en');
    Object.defineProperty(URL, 'createObjectURL', {
      configurable: true,
      value: vi.fn(() => 'blob:tts-player'),
    });
    Object.defineProperty(URL, 'revokeObjectURL', {
      configurable: true,
      value: vi.fn(),
    });
  });

  it('requires a manual click and renders a non-autoplay audio control', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(new Uint8Array([0x52, 0x49, 0x46, 0x46]), {
        status: 200,
        headers: { 'Content-Type': 'audio/wav' },
      }),
    );
    const root = document.createElement('div');
    act(() => render(<TtsPlayer text="Read this" />, root));

    expect(fetchMock).not.toHaveBeenCalled();
    const button = root.querySelector<HTMLButtonElement>('[data-testid="tts-synthesize"]');
    expect(button?.textContent).toContain('Read aloud');
    await act(async () => {
      button?.click();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    const audio = root.querySelector<HTMLAudioElement>('audio');
    expect(audio).not.toBeNull();
    expect(audio?.controls).toBe(true);
    expect(audio?.autoplay).toBe(false);
    expect(audio?.src).toContain('blob:tts-player');
    act(() => render(null, root));
  });
});
