import { describe, expect, it } from 'vitest';

import { normalizeTtsSettings, ttsSettingsPatch } from './ttsSettings';

describe('TTS settings helpers', () => {
  it('overlays defaults without discarding existing user choices', () => {
    expect(normalizeTtsSettings({ enabled: false, voice: 'studio.zh' })).toEqual({
      enabled: false,
      provider: 'local_scheduler',
      task: 'kb.speech.synthesize',
      voice: 'studio.zh',
      language: 'auto',
      speed: 1,
      format: 'wav',
    });
  });

  it('builds only the editable delta and leaves scheduler ownership fixed', () => {
    expect(
      ttsSettingsPatch({ enabled: true, voice: 'default', language: 'zh-CN', speed: 1.2 }),
    ).toEqual({
      tts: { enabled: true, voice: 'default', language: 'zh-CN', speed: 1.2 },
    });
  });
});
