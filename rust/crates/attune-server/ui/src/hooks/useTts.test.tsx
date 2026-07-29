import { render } from 'preact';
import { act } from 'preact/test-utils';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useTts, type TtsController } from './useTts';

let controller: TtsController | undefined;

function Harness({ text }: { text: string }) {
  controller = useTts(text);
  return null;
}

describe('useTts', () => {
  beforeEach(() => {
    controller = undefined;
    sessionStorage.clear();
    vi.restoreAllMocks();
    Object.defineProperty(URL, 'createObjectURL', {
      configurable: true,
      value: vi.fn(() => 'blob:attune-tts'),
    });
    Object.defineProperty(URL, 'revokeObjectURL', {
      configurable: true,
      value: vi.fn(),
    });
  });

  it('creates a WAV object URL without autoplay and revokes it when text changes', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(new Uint8Array([0x52, 0x49, 0x46, 0x46]), {
        status: 200,
        headers: { 'Content-Type': 'audio/wav' },
      }),
    );
    const play = vi.spyOn(HTMLMediaElement.prototype, 'play');
    const root = document.createElement('div');

    act(() => render(<Harness text={'  \u00a0hello\u2003  '} />, root));
    await act(async () => controller?.synthesize());

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, options] = fetchMock.mock.calls[0];
    expect(url).toBe('/api/v1/tts/synthesize');
    expect(options?.method).toBe('POST');
    expect(options?.body).toBe(JSON.stringify({ text: '\u00a0hello\u2003' }));
    expect(controller?.status).toBe('ready');
    expect(controller?.audioUrl).toBe('blob:attune-tts');
    expect(play).not.toHaveBeenCalled();

    act(() => render(<Harness text="changed" />, root));
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:attune-tts');
    expect(controller?.status).toBe('idle');
    expect(controller?.audioUrl).toBeNull();
    act(() => render(null, root));
  });

  it('treats NBSP as semantic text instead of broad-trimming it away', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(new Uint8Array([0x52, 0x49, 0x46, 0x46]), {
        status: 200,
        headers: { 'Content-Type': 'audio/wav' },
      }),
    );
    const root = document.createElement('div');
    act(() => render(<Harness text={'\u00a0'} />, root));

    await act(async () => controller?.synthesize());

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][1]?.body).toBe(JSON.stringify({ text: '\u00a0' }));
    expect(controller?.status).toBe('ready');
    act(() => render(null, root));
  });

  it('does not submit text the server deterministically rejects', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch');
    const root = document.createElement('div');

    for (const text of ['字'.repeat(129), 'first line\nsecond line']) {
      act(() => render(<Harness text={text} />, root));
      await act(async () => controller?.synthesize());
      expect(controller?.status).toBe('idle');
    }

    expect(fetchMock).not.toHaveBeenCalled();
    act(() => render(null, root));
  });

  it('exposes an error and allows a manual retry', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(
        new Response('{"error":"scheduler unavailable"}', {
          status: 503,
          headers: { 'Content-Type': 'application/json' },
        }),
      )
      .mockResolvedValueOnce(
        new Response(new Uint8Array([0x52, 0x49, 0x46, 0x46]), {
          status: 200,
          headers: { 'Content-Type': 'audio/wav' },
        }),
      );
    const root = document.createElement('div');
    act(() => render(<Harness text="retry me" />, root));

    await act(async () => controller?.synthesize());
    expect(controller?.status).toBe('error');
    expect(controller?.error).toContain('503');

    await act(async () => controller?.synthesize());
    expect(controller?.status).toBe('ready');
    expect(controller?.error).toBeNull();
    act(() => render(null, root));
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:attune-tts');
  });
});
