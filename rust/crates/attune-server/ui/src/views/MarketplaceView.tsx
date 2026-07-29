/** MarketplaceView · G3 (2026-05-01) PluginHub 插件市场
 *
 * 列出 hub 上对当前 license 可见的插件 + 支持启动 trial / 安装。
 *
 * Backend: /api/v1/marketplace/plugins (GET) + /install (POST)
 * 未配置会员时走离线目录；用户在 Settings 登录或配置授权码后切真 PluginHub。
 */

import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { api } from '../store/api';
import { toast } from '../components';
import { confirmDialog } from '../components/ConfirmModal';
import { t } from '../i18n';
import { currentView, memberVertical, settingsInitialTab } from '../store/signals';
import { verticalLabel } from '../hooks/useMember';

interface PluginListing {
  id: string;
  name: string;
  type: string;
  category: string;
  description: string;
  latest_version: string;
  tags: string[];
  min_plan: string;
  available: boolean;
  trial_available: boolean;
  trial_days: number;
}

interface ListResponse {
  hub_version: string;
  user_plan: string;
  upgrade_url: string;
  plugins: PluginListing[];
  provider: string;
  installed_versions?: Record<string, string>;
}

interface InstallResponse {
  install_id: number;
  plugin_id: string;
  version: string;
  trial_started?: string;
  trial_expires?: string;
}

/** plan id → 本地化标签；未知 plan 原样返回 */
function planLabel(plan: string): string {
  if (plan === 'individual') return t('market.plan.individual');
  if (plan === 'pro') return t('market.plan.pro');
  if (plan === 'enterprise') return t('market.plan.enterprise');
  return plan;
}

function providerLabel(provider: string): string {
  if (provider === 'mock') return t('market.provider.offline');
  if (provider === 'real-hub') return t('market.provider.official');
  return provider;
}

function compareVersions(a: string, b: string): number {
  const left = a.split(/[.-]/).map((p) => Number.parseInt(p, 10));
  const right = b.split(/[.-]/).map((p) => Number.parseInt(p, 10));
  const len = Math.max(left.length, right.length);
  for (let i = 0; i < len; i += 1) {
    const av = Number.isFinite(left[i]) ? left[i] : 0;
    const bv = Number.isFinite(right[i]) ? right[i] : 0;
    if (av !== bv) return av > bv ? 1 : -1;
  }
  return a.localeCompare(b);
}

export function MarketplaceView(): JSX.Element {
  const [data, setData] = useState<ListResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [installing, setInstalling] = useState<string | null>(null);
  const [uninstalling, setUninstalling] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState('all');
  const [statusFilter, setStatusFilter] = useState('all');

  async function load() {
    setLoading(true);
    setError(null);
    try {
      const resp = await api.get<ListResponse>('/marketplace/plugins');
      setData(resp);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load();
  }, []);

  function openMemberSettings(): void {
    settingsInitialTab.value = 'member';
    currentView.value = 'settings';
  }

  function openUpgrade(): void {
    if (data?.upgrade_url) {
      window.open(data.upgrade_url, '_blank', 'noopener');
      return;
    }
    openMemberSettings();
  }

  async function install(plugin: PluginListing) {
    setInstalling(plugin.id);
    try {
      const resp = await api.post<InstallResponse>(
        `/marketplace/plugins/${plugin.id}/install`,
        {},
      );
      const trialMsg = resp.trial_expires
        ? t('market.toast.trial_until', {
            date: new Date(resp.trial_expires).toLocaleDateString('zh-CN'),
          })
        : '';
      toast(
        'success',
        t('market.toast.installed', {
          name: plugin.name,
          version: resp.version,
          trial: trialMsg,
        }),
      );
      // Reload listing 让 trial 状态更新
      await load();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg.includes('plan_required') || msg.includes('402')) {
        toast(
          'error',
          t('market.toast.plan_required', {
            name: plugin.name,
            plan: planLabel(plugin.min_plan),
            url: data?.upgrade_url ?? '',
          }),
        );
      } else if (msg.includes('pluginhub_not_configured')) {
        toast('error', t('market.toast.pluginhub_not_configured'));
      } else {
        toast('error', t('market.toast.install_failed', { message: msg }));
      }
    } finally {
      setInstalling(null);
    }
  }

  async function uninstall(plugin: PluginListing) {
    const ok = await confirmDialog({
      title: t('confirm.title.uninstallPlugin'),
      message: t('market.confirm.uninstall', { name: plugin.name }),
      danger: true,
    });
    if (!ok) return;
    setUninstalling(plugin.id);
    try {
      await api.delete(`/plugins/${encodeURIComponent(plugin.id)}`);
      toast('success', t('market.toast.uninstalled', { name: plugin.name }));
      await load();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      toast('error', t('market.toast.uninstall_failed', { message: msg }));
    } finally {
      setUninstalling(null);
    }
  }

  if (loading) {
    return (
      <div style={{ padding: 'var(--space-5)', textAlign: 'center', color: 'var(--color-text-secondary)' }}>
        {t('market.loading')}
      </div>
    );
  }

  if (error || !data) {
    return (
      <div style={{ padding: 'var(--space-5)' }}>
        <div style={{ color: 'var(--color-error)' }}>
          {t('market.load_failed', { message: error ?? t('market.no_data') })}
        </div>
        <button onClick={() => void load()} style={{ marginTop: 'var(--space-3)' }}>
          {t('common.retry')}
        </button>
      </div>
    );
  }

  const installedVersions = data.installed_versions ?? {};
  const categories = Array.from(new Set(data.plugins.map((p) => p.category).filter(Boolean))).sort((a, b) =>
    a.localeCompare(b),
  );
  const normalizedQuery = query.trim().toLowerCase();
  const filteredPlugins = data.plugins.filter((p) => {
    const installedVersion = installedVersions[p.id];
    const updateAvailable = Boolean(installedVersion && compareVersions(p.latest_version, installedVersion) > 0);
    if (category !== 'all' && p.category !== category) return false;
    if (statusFilter === 'available' && !p.available) return false;
    if (statusFilter === 'installed' && !installedVersion) return false;
    if (statusFilter === 'updates' && !updateAvailable) return false;
    if (!normalizedQuery) return true;
    const haystack = [p.id, p.name, p.category, p.description, ...p.tags].join(' ').toLowerCase();
    return haystack.includes(normalizedQuery);
  });

  return (
    <div style={{ padding: 'var(--space-6)', maxWidth: 1200, margin: '0 auto' }}>
      <header style={{ marginBottom: 'var(--space-5)', display: 'flex', justifyContent: 'space-between', alignItems: 'baseline' }}>
        <div>
          <h1 style={{ fontSize: 'var(--text-2xl)', fontWeight: 600, margin: 0 }}>{t('market.title')}</h1>
          <div style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-sm)', marginTop: 'var(--space-1)' }}>
            {t('market.current_plan')}<strong>{planLabel(data.user_plan)}</strong>
            {' · '}
            {t('market.provider')}<code>{providerLabel(data.provider)}</code>
            {' · '}
            {t('market.hub_version', { version: data.hub_version })}
          </div>
          {/* GAP-B: cloud 下发的会员场景 (vertical) — 纯展示文案,不参与门禁。 */}
          {memberVertical.value && (
            <div style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-sm)', marginTop: 'var(--space-1)' }}>
              {t('market.current_vertical', { vertical: verticalLabel(memberVertical.value) })}
            </div>
          )}
        </div>
        {data.user_plan === 'individual' && (
          <button
            type="button"
            onClick={openUpgrade}
            style={{
              padding: 'var(--space-2) var(--space-3)',
              background: 'var(--color-accent)',
              color: 'white',
              border: 'none',
              borderRadius: 'var(--radius-sm)',
              fontSize: 'var(--text-sm)',
              cursor: 'pointer',
            }}
          >
            {t('market.upgrade_to_pro')}
          </button>
        )}
        {data.provider === 'mock' && (
          <button
            type="button"
            onClick={openMemberSettings}
            style={{
              padding: 'var(--space-2) var(--space-3)',
              background: 'transparent',
              color: 'var(--color-accent)',
              border: '1px solid var(--color-accent)',
              borderRadius: 'var(--radius-sm)',
              fontSize: 'var(--text-sm)',
              cursor: 'pointer',
            }}
          >
            {t('market.configure_member')}
          </button>
        )}
      </header>

      {data.plugins.length === 0 ? (
        <div style={{ color: 'var(--color-text-secondary)' }}>
          {t('market.empty.before')}
          <strong>{t('market.empty.config_path')}</strong>
          {t('market.empty.after')}
        </div>
      ) : (
        <>
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 180px), 1fr))',
              gap: 'var(--space-3)',
              marginBottom: 'var(--space-4)',
              alignItems: 'center',
            }}
          >
            <input
              value={query}
              onInput={(e) => setQuery((e.target as HTMLInputElement).value)}
              placeholder={t('market.search.placeholder')}
              style={{
                width: '100%',
                boxSizing: 'border-box',
                padding: 'var(--space-2)',
                borderRadius: 'var(--radius-sm)',
                border: '1px solid var(--color-border)',
                background: 'var(--color-bg)',
                color: 'var(--color-text)',
                fontSize: 'var(--text-sm)',
              }}
            />
            <select
              value={category}
              onChange={(e) => setCategory((e.target as HTMLSelectElement).value)}
              style={{
                width: '100%',
                boxSizing: 'border-box',
                padding: 'var(--space-2)',
                borderRadius: 'var(--radius-sm)',
                border: '1px solid var(--color-border)',
                background: 'var(--color-bg)',
                color: 'var(--color-text)',
                fontSize: 'var(--text-sm)',
              }}
            >
              <option value="all">{t('market.filter.category_all')}</option>
              {categories.map((item) => (
                <option key={item} value={item}>{item}</option>
              ))}
            </select>
            <select
              value={statusFilter}
              onChange={(e) => setStatusFilter((e.target as HTMLSelectElement).value)}
              style={{
                width: '100%',
                boxSizing: 'border-box',
                padding: 'var(--space-2)',
                borderRadius: 'var(--radius-sm)',
                border: '1px solid var(--color-border)',
                background: 'var(--color-bg)',
                color: 'var(--color-text)',
                fontSize: 'var(--text-sm)',
              }}
            >
              <option value="all">{t('market.filter.status_all')}</option>
              <option value="available">{t('market.filter.available')}</option>
              <option value="installed">{t('market.filter.installed')}</option>
              <option value="updates">{t('market.filter.updates')}</option>
            </select>
          </div>

          {filteredPlugins.length === 0 ? (
            <div style={{ color: 'var(--color-text-secondary)' }}>{t('market.filter.empty')}</div>
          ) : (
            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(min(100%, 320px), 1fr))', gap: 'var(--space-4)' }}>
          {filteredPlugins.map((p) => {
            const installedVersion = installedVersions[p.id];
            const updateAvailable = Boolean(installedVersion && compareVersions(p.latest_version, installedVersion) > 0);
            return (
            <article
              key={p.id}
              style={{
                background: 'var(--color-surface-elevated)',
                border: '1px solid var(--color-border)',
                borderRadius: 'var(--radius-md)',
                padding: 'var(--space-4)',
                display: 'flex',
                flexDirection: 'column',
                gap: 'var(--space-3)',
              }}
            >
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start' }}>
                <div>
                  <h3 style={{ margin: 0, fontSize: 'var(--text-lg)', fontWeight: 600 }}>{p.name}</h3>
                  <div style={{ fontSize: 'var(--text-xs)', color: 'var(--color-text-secondary)', marginTop: 'var(--space-1)' }}>
                    {p.id} · v{p.latest_version} · {p.category}
                  </div>
                  {installedVersion && (
                    <div style={{ fontSize: 'var(--text-xs)', color: updateAvailable ? 'var(--color-warning)' : 'var(--color-success)', marginTop: 'var(--space-1)' }}>
                      {updateAvailable
                        ? t('market.update_available', { current: installedVersion, latest: p.latest_version })
                        : t('market.installed_version', { version: installedVersion })}
                    </div>
                  )}
                </div>
                <span
                  style={{
                    padding: '2px 8px',
                    fontSize: 'var(--text-xs)',
                    background: p.available ? 'var(--color-success-bg)' : 'var(--color-warning-bg)',
                    color: p.available ? 'var(--color-success)' : 'var(--color-warning)',
                    borderRadius: 'var(--radius-sm)',
                  }}
                >
                  {p.available
                    ? t('market.plan.available', { plan: planLabel(p.min_plan) })
                    : t('market.plan.required', { plan: planLabel(p.min_plan) })}
                </span>
              </div>

              <p style={{ margin: 0, fontSize: 'var(--text-sm)', color: 'var(--color-text-secondary)', flex: 1 }}>
                {p.description}
              </p>

              <div style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'center', flexWrap: 'wrap' }}>
                {p.tags.map((t) => (
                  <span
                    key={t}
                    style={{
                      padding: '2px 6px',
                      fontSize: 'var(--text-xs)',
                      background: 'var(--color-bg)',
                      color: 'var(--color-text-secondary)',
                      borderRadius: 'var(--radius-sm)',
                      border: '1px solid var(--color-border)',
                    }}
                  >
                    {t}
                  </span>
                ))}
              </div>

              <div style={{ display: 'flex', gap: 'var(--space-2)', marginTop: 'auto' }}>
                {p.available ? (
                  <button
                    onClick={() => void install(p)}
                    disabled={installing === p.id}
                    style={{
                      flex: 1,
                      padding: 'var(--space-2)',
                      background: 'var(--color-accent)',
                      color: 'white',
                      border: 'none',
                      borderRadius: 'var(--radius-sm)',
                      cursor: installing === p.id ? 'wait' : 'pointer',
                      opacity: installing === p.id ? 0.6 : 1,
                    }}
                  >
                    {installing === p.id
                      ? t('market.installing')
                      : updateAvailable
                        ? t('market.update')
                        : installedVersion
                          ? t('market.reinstall')
                          : t('market.install')}
                  </button>
                ) : p.trial_available ? (
                  <button
                    onClick={() => void install(p)}
                    disabled={installing === p.id}
                    style={{
                      flex: 1,
                      padding: 'var(--space-2)',
                      background: 'transparent',
                      color: 'var(--color-accent)',
                      border: '1px solid var(--color-accent)',
                      borderRadius: 'var(--radius-sm)',
                      cursor: installing === p.id ? 'wait' : 'pointer',
                      opacity: installing === p.id ? 0.6 : 1,
                    }}
                  >
                    {installing === p.id
                      ? t('market.trial_starting')
                      : t('market.trial', { days: p.trial_days })}
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={openMemberSettings}
                    style={{
                      flex: 1,
                      padding: 'var(--space-2)',
                      background: 'var(--color-bg)',
                      color: 'var(--color-text)',
                      border: '1px solid var(--color-border)',
                      borderRadius: 'var(--radius-sm)',
                      textAlign: 'center',
                      cursor: 'pointer',
                    }}
                  >
                    {t('market.upgrade_required')}
                  </button>
                )}
                {installedVersion && (
                  <button
                    type="button"
                    onClick={() => void uninstall(p)}
                    disabled={uninstalling === p.id}
                    style={{
                      flex: 1,
                      padding: 'var(--space-2)',
                      background: 'var(--color-bg)',
                      color: 'var(--color-error)',
                      border: '1px solid var(--color-error)',
                      borderRadius: 'var(--radius-sm)',
                      textAlign: 'center',
                      cursor: uninstalling === p.id ? 'wait' : 'pointer',
                      opacity: uninstalling === p.id ? 0.6 : 1,
                    }}
                  >
                    {uninstalling === p.id ? t('market.uninstalling') : t('market.uninstall')}
                  </button>
                )}
              </div>
            </article>
            );
          })}
            </div>
          )}
        </>
      )}
    </div>
  );
}
