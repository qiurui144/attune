/** useSuggestions — 零成本主动建议卡(被动可见,可忽略/可静音)。
 *
 *  对接 GET /api/v1/suggestions(确定性引擎实时重算,零 LLM)+
 *  POST /api/v1/suggestions/{id}/dismiss + POST /api/v1/suggestions/mute。
 *  建议卡是被动呈现:不推送、不弹窗、不红点(per 成本契约 + spec §2 不做)。
 */
import { signal } from '@preact/signals';
import { api } from '../store/api';

export type SuggestionCostHint = { tier: 'free' | 'local' | 'cloud'; note: string };
export type SuggestionCard = {
  id: string;
  signature: string;
  kind: 'organize' | 'enrich' | 'retrieval' | 'profile';
  title: string;
  detail: string;
  ref_ids: string[];
  action_kind: 'open_organize_wizard' | 'open_search' | 'run_skill_evolution' | 'open_profile';
  cost_hint: SuggestionCostHint;
};

export const suggestionCards = signal<SuggestionCard[]>([]);
export const suggestionsLoading = signal(false);

export async function loadSuggestions(): Promise<void> {
  suggestionsLoading.value = true;
  try {
    const raw = await api.get<{ cards?: SuggestionCard[] }>('/suggestions');
    suggestionCards.value = raw.cards ?? [];
  } catch {
    // 引擎不可用 → 退化为"无建议卡"(=当前行为),不打扰用户。
    suggestionCards.value = [];
  } finally {
    suggestionsLoading.value = false;
  }
}

export async function dismissSuggestion(id: string): Promise<void> {
  // 乐观移除 + 后端持久化(幂等)。
  suggestionCards.value = suggestionCards.value.filter((c) => c.id !== id);
  try {
    await api.post(`/suggestions/${encodeURIComponent(id)}/dismiss`);
  } catch {
    // 失败时下次 load 会重新拉到(后端是 SSOT),不阻塞。
  }
}

export async function muteSuggestionKind(kind: SuggestionCard['kind']): Promise<void> {
  suggestionCards.value = suggestionCards.value.filter((c) => c.kind !== kind);
  try {
    await api.post('/suggestions/mute', { kind });
  } catch {
    /* 同 dismiss:后端 SSOT,下次 load 自纠 */
  }
}
