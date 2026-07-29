import { describe, expect, it } from 'vitest';

import {
  detectLlmPreset,
  isCloudNetworkEndpoint,
  isLlmConfigLocal,
  isLocalNetworkEndpoint,
  isSchedulerEndpoint,
  modelCatalogKey,
  schedulerNativeBase,
  schedulerOpenAiEndpoint,
} from './llmConfig';

describe('detectLlmPreset', () => {
  it('restores scheduler presets from native providers and legacy port-8090 endpoints', () => {
    expect(detectLlmPreset('local_scheduler', 'http://10.0.0.8:19090')).toBe('local_scheduler');
    expect(detectLlmPreset('EDGE_SCHEDULER', 'http://192.168.1.5:19090')).toBe('local_scheduler');
    expect(detectLlmPreset('openai_compat', 'http://127.0.0.1:8090')).toBe('local_scheduler');
    expect(detectLlmPreset('openai_compat', 'http://192.168.1.5:8090/v1/')).toBe('local_scheduler');
  });

  it('restores known cloud presets while tolerating trailing slashes', () => {
    expect(detectLlmPreset('openai_compat', 'https://api.openai.com/v1/')).toBe('openai');
    expect(detectLlmPreset('deepseek', 'https://api.deepseek.com/v1')).toBe('deepseek');
    expect(detectLlmPreset('qwen', 'https://dashscope.aliyuncs.com/compatible-mode/v1/')).toBe('qwen');
  });

  it('does not treat a disguised public hostname as the local scheduler', () => {
    expect(detectLlmPreset('openai_compat', 'http://localhost.evil.test:8090/v1')).toBe('custom');
    expect(detectLlmPreset('openai_compat', 'http://10.0.0.1@evil.test:8090/v1')).toBe('custom');
  });
});

describe('LLM destination classification', () => {
  it('distinguishes cloud and local openai_compat endpoints', () => {
    expect(isLlmConfigLocal('openai_compat', 'https://api.openai.com/v1')).toBe(false);
    expect(isLlmConfigLocal('openai_compat', 'http://127.0.0.1:8090/v1')).toBe(true);
    expect(isLlmConfigLocal('openai_compat', 'http://192.168.50.4:8090/v1')).toBe(true);
    expect(isLlmConfigLocal('openai_compat', 'http://[fd00::2]:8090/v1')).toBe(true);
  });

  it('honors an explicit local provider even before an endpoint is configured', () => {
    expect(isLlmConfigLocal('scheduler_native', undefined)).toBe(true);
    expect(isLlmConfigLocal('local', null)).toBe(true);
  });

  it('returns unknown for incomplete or malformed non-local configuration', () => {
    expect(isLlmConfigLocal(undefined, undefined)).toBeNull();
    expect(isLlmConfigLocal('openai_compat', '')).toBeNull();
    expect(isLlmConfigLocal('openai_compat', 'not a url')).toBeNull();
  });

  it('matches the server local-network and scheduler-port boundary', () => {
    for (const endpoint of [
      'http://localhost:8090/v1',
      'http://0.0.0.0:8090/v1',
      'http://10.2.3.4:8090/v1',
      'http://172.31.2.3:8090/v1',
      'http://169.254.1.2:8090/v1',
      'http://[::1]:8090/v1',
      'http://[fe80::2]:8090/v1',
    ]) {
      expect(isLocalNetworkEndpoint(endpoint), endpoint).toBe(true);
      expect(isCloudNetworkEndpoint(endpoint), endpoint).toBe(false);
      expect(isSchedulerEndpoint(endpoint), endpoint).toBe(true);
    }

    for (const endpoint of [
      'https://api.openai.com/v1',
      'http://8.8.8.8:8090/v1',
      'http://localhost.evil.test:8090/v1',
      'ftp://127.0.0.1:8090/v1',
    ]) {
      expect(isLocalNetworkEndpoint(endpoint), endpoint).toBe(false);
      expect(isCloudNetworkEndpoint(endpoint), endpoint).toBe(endpoint.startsWith('http'));
      expect(isSchedulerEndpoint(endpoint), endpoint).toBe(false);
    }
  });
});

describe('modelCatalogKey', () => {
  it('maps exact OpenAI-compatible vendor hosts to their model catalogs', () => {
    expect(modelCatalogKey('openai_compat', 'https://api.openai.com/v1')).toBe('openai');
    expect(modelCatalogKey(' OPENAI_COMPAT ', 'https://api.openai.com/v1')).toBe('openai');
    expect(modelCatalogKey('openai_compat', 'https://api.deepseek.com/v1')).toBe('deepseek');
    expect(modelCatalogKey('openai_compat', 'https://dashscope.aliyuncs.com/compatible-mode/v1')).toBe('qwen');
    expect(modelCatalogKey('openai_compat', 'https://generativelanguage.googleapis.com/v1beta/openai')).toBe('gemini');
  });

  it('does not infer a catalog from a hostname substring or URL path', () => {
    expect(modelCatalogKey('openai_compat', 'https://api.openai.com.evil.test/v1')).toBe('openai_compat');
    expect(modelCatalogKey('openai_compat', 'https://gateway.test/api.openai.com/v1')).toBe('openai_compat');
  });
});

describe('schedulerOpenAiEndpoint', () => {
  it('round-trips scheduler-native and OpenAI adapter bases', () => {
    expect(schedulerOpenAiEndpoint('http://127.0.0.1:8090')).toBe('http://127.0.0.1:8090/v1');
    expect(schedulerOpenAiEndpoint('http://127.0.0.1:8090/')).toBe('http://127.0.0.1:8090/v1');
    expect(schedulerOpenAiEndpoint('http://127.0.0.1:8090/v1/')).toBe('http://127.0.0.1:8090/v1');
    expect(schedulerNativeBase('http://127.0.0.1:8090/v1/')).toBe('http://127.0.0.1:8090');
  });
});
