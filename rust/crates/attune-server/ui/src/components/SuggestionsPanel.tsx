/** SuggestionsPanel · 零成本主动建议卡区域(被动可见)
 *
 * 北极星"忙碌时的助理"被动腿:确定性引擎生成的建议卡,固定区域被动呈现,
 * 不推送、不弹窗、不红点。每张卡可忽略(dismiss)或永久关闭该类(mute)。
 * 点击卡 action 路由到既有显式 handler(整理向导 / 搜索 / 技能进化 / 画像),
 * 那里才带成本 UI —— 卡本身生成永远零成本(cost_hint.tier 标注点击后成本)。
 *
 * per spec docs/superpowers/specs/2026-06-17-suggestions-and-thirdparty-accounts.md
 * 能力 A §UI。
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { t } from '../i18n';
import { currentView } from '../store/signals';
import {
  suggestionCards,
  suggestionsLoading,
  loadSuggestions,
  dismissSuggestion,
  muteSuggestionKind,
  type SuggestionCard,
} from '../hooks/useSuggestions';

const COST_TIER_KEY: Record<SuggestionCard['cost_hint']['tier'], string> = {
  free: 'suggestions.cost.free',
  local: 'suggestions.cost.local',
  cloud: 'suggestions.cost.cloud',
};

function routeAction(card: SuggestionCard): void {
  // action 全部指向既有显式 handler。最小落地:切到对应视图(用户在那里完成显式操作)。
  switch (card.action_kind) {
    case 'open_organize_wizard':
      currentView.value = 'projects';
      break;
    case 'open_search':
      currentView.value = 'knowledge';
      break;
    case 'run_skill_evolution':
      currentView.value = 'skills';
      break;
    case 'open_profile':
      currentView.value = 'privacy';
      break;
    case 'open_accounts_panel':
      // 第三方账号管理面板挂在 Remote 视图(波 C)。
      currentView.value = 'remote';
      break;
  }
}

export function SuggestionsPanel(): JSX.Element | null {
  useEffect(() => {
    void loadSuggestions();
  }, []);

  const cards = suggestionCards.value;
  // 无卡 = 被动退化为空(不占位、不催促)。
  if (suggestionsLoading.value && cards.length === 0) return null;
  if (cards.length === 0) return null;

  return (
    <section class="suggestions-panel" aria-label={t('suggestions.title')}>
      <h3 class="suggestions-panel__title">{t('suggestions.title')}</h3>
      <ul class="suggestions-panel__list">
        {cards.map((card) => (
          <li key={card.id} class="suggestion-card">
            <button
              type="button"
              class="suggestion-card__main"
              onClick={() => routeAction(card)}
              title={card.cost_hint.note}
            >
              <span class="suggestion-card__badge">{t('suggestions.badge')}</span>
              <span class="suggestion-card__cardtitle">{card.title}</span>
              {card.detail && <span class="suggestion-card__detail">{card.detail}</span>}
              <span class="suggestion-card__cost">
                {t(COST_TIER_KEY[card.cost_hint.tier])} · {card.cost_hint.note}
              </span>
            </button>
            <div class="suggestion-card__actions">
              <button
                type="button"
                class="suggestion-card__dismiss"
                onClick={() => void dismissSuggestion(card.id)}
                title={t('suggestions.dismiss')}
                aria-label={t('suggestions.dismiss')}
              >
                {t('suggestions.dismiss')}
              </button>
              <button
                type="button"
                class="suggestion-card__mute"
                onClick={() => void muteSuggestionKind(card.kind)}
                title={t('suggestions.mute')}
                aria-label={t('suggestions.mute')}
              >
                {t('suggestions.mute')}
              </button>
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
