export type TtsSettingsValue = {
  enabled: boolean;
  provider: 'local_scheduler';
  task: 'kb.speech.synthesize';
  voice: string;
  language: 'auto' | 'zh-CN' | 'en-US';
  speed: number;
  format: 'wav';
};

export type EditableTtsSettings = Pick<
  TtsSettingsValue,
  'enabled' | 'voice' | 'language' | 'speed'
>;

export function normalizeTtsSettings(value: unknown): TtsSettingsValue {
  const source = value && typeof value === 'object' ? value as Record<string, unknown> : {};
  const language = source.language === 'zh-CN' || source.language === 'en-US'
    ? source.language
    : 'auto';
  const speed = typeof source.speed === 'number'
    && Number.isFinite(source.speed)
    && source.speed >= 0.5
    && source.speed <= 2
    ? source.speed
    : 1;
  return {
    enabled: typeof source.enabled === 'boolean' ? source.enabled : true,
    provider: 'local_scheduler',
    task: 'kb.speech.synthesize',
    voice: typeof source.voice === 'string' && source.voice ? source.voice : 'auto',
    language,
    speed,
    format: 'wav',
  };
}

export function ttsSettingsPatch(value: EditableTtsSettings): { tts: EditableTtsSettings } {
  return {
    tts: {
      enabled: value.enabled,
      voice: value.voice,
      language: value.language,
      speed: value.speed,
    },
  };
}
