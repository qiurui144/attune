import { useCallback, useEffect, useRef, useState } from 'preact/hooks';

import { apiBlob } from '../store/api';
import { ttsRequestText } from '../ttsText';

export type TtsStatus = 'idle' | 'loading' | 'ready' | 'error';

export type TtsController = {
  status: TtsStatus;
  audioUrl: string | null;
  error: string | null;
  synthesize: () => Promise<void>;
};

export function useTts(text: string): TtsController {
  const [status, setStatus] = useState<TtsStatus>('idle');
  const [audioUrl, setAudioUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const audioUrlRef = useRef<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const generationRef = useRef(0);

  const revokeAudioUrl = useCallback((): void => {
    if (audioUrlRef.current) {
      URL.revokeObjectURL(audioUrlRef.current);
      audioUrlRef.current = null;
    }
    setAudioUrl(null);
  }, []);

  useEffect(() => {
    generationRef.current += 1;
    abortRef.current?.abort();
    abortRef.current = null;
    revokeAudioUrl();
    setError(null);
    setStatus('idle');
    return () => {
      generationRef.current += 1;
      abortRef.current?.abort();
      abortRef.current = null;
      if (audioUrlRef.current) {
        URL.revokeObjectURL(audioUrlRef.current);
        audioUrlRef.current = null;
      }
    };
  }, [text, revokeAudioUrl]);

  const synthesize = useCallback(async (): Promise<void> => {
    const requestText = ttsRequestText(text);
    if (requestText === null) return;
    generationRef.current += 1;
    const generation = generationRef.current;
    abortRef.current?.abort();
    revokeAudioUrl();
    const controller = new AbortController();
    abortRef.current = controller;
    setError(null);
    setStatus('loading');
    try {
      const blob = await apiBlob('/tts/synthesize', {
        method: 'POST',
        body: JSON.stringify({ text: requestText }),
        signal: controller.signal,
      });
      if (generationRef.current !== generation || controller.signal.aborted) return;
      if (blob.type !== 'audio/wav' && blob.type !== 'audio/x-wav') {
        throw new Error(`Unexpected TTS content type: ${blob.type || 'missing'}`);
      }
      const url = URL.createObjectURL(blob);
      audioUrlRef.current = url;
      setAudioUrl(url);
      setStatus('ready');
    } catch (cause) {
      if (generationRef.current !== generation || controller.signal.aborted) return;
      setError(cause instanceof Error ? cause.message : String(cause));
      setStatus('error');
    } finally {
      if (abortRef.current === controller) abortRef.current = null;
    }
  }, [text, revokeAudioUrl]);

  return { status, audioUrl, error, synthesize };
}
