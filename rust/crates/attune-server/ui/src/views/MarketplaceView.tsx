/** MarketplaceView · G3 (2026-05-01) PluginHub 插件市场
 *
 * 列出 hub 上对当前 license 可见的插件 + 支持启动 trial / 安装。
 *
 * Backend: /api/v1/marketplace/plugins (GET) + /install (POST)
 * 默认走内嵌 Mock provider（4 个 attune-pro vertical plugin）；
 * 用户在 Settings 配 pluginhub.url + license_key 后切真 hub.attune.ai。
 */

import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { api } from '../store/api';
import { toast } from '../components';

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
}

interface InstallResponse {
  install_id: number;
  plugin_id: string;
  version: string;
  trial_started?: string;
  trial_expires?: string;
  download_url: string;
}

const PLAN_LABELS: Record<string, string> = {
  individual: '免费',
  pro: 'Pro',
  enterprise: '企业版',
};

export function MarketplaceView(): JSX.Element {
  const [data, setData] = useState<ListResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [installing, setInstalling] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

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

  async function install(plugin: PluginListing) {
    setInstalling(plugin.id);
    try {
      const resp = await api.post<InstallResponse>(
        `/marketplace/plugins/${plugin.id}/install`,
        {},
      );
      const trialMsg = resp.trial_expires
        ? ` — 试用至 ${new Date(resp.trial_expires).toLocaleDateString('zh-CN')}`
        : '';
      toast('success', `${plugin.name} v${resp.version} 已安装${trialMsg}`);
      // Reload listing 让 trial 状态更新
      await load();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg.includes('plan_required') || msg.includes('402')) {
        toast(
          'error',
          `${plugin.name} 需要 ${PLAN_LABELS[plugin.min_plan] ?? plugin.min_plan} 会员。访问 ${data?.upgrade_url} 升级。`,
        );
      } else {
        toast('error', `安装失败: ${msg}`);
      }
    } finally {
      setInstalling(null);
    }
  }

  if (loading) {
    return (
      <div style={{ padding: 'var(--space-5)', textAlign: 'center', color: 'var(--color-text-secondary)' }}>
        正在加载插件市场...
      </div>
    );
  }

  if (error || !data) {
    return (
      <div style={{ padding: 'var(--space-5)' }}>
        <div style={{ color: 'var(--color-error)' }}>加载失败：{error ?? '无数据'}</div>
        <button onClick={() => void load()} style={{ marginTop: 'var(--space-3)' }}>
          重试
        </button>
      </div>
    );
  }

  return (
    <div style={{ padding: 'var(--space-5)', maxWidth: 1200, margin: '0 auto' }}>
      <header style={{ marginBottom: 'var(--space-5)', display: 'flex', justifyContent: 'space-between', alignItems: 'baseline' }}>
        <div>
          <h1 style={{ fontSize: 'var(--text-2xl)', fontWeight: 600, margin: 0 }}>插件市场</h1>
          <div style={{ color: 'var(--color-text-secondary)', fontSize: 'var(--text-sm)', marginTop: 'var(--space-1)' }}>
            当前会员：<strong>{PLAN_LABELS[data.user_plan] ?? data.user_plan}</strong>
            {' · '}
            提供商：<code>{data.provider}</code>
            {' · '}
            hub v{data.hub_version}
          </div>
        </div>
        {data.user_plan === 'individual' && (
          <a
            href={data.upgrade_url}
            target="_blank"
            rel="noopener"
            style={{
              padding: 'var(--space-2) var(--space-3)',
              background: 'var(--color-accent)',
              color: 'white',
              borderRadius: 'var(--radius-sm)',
              textDecoration: 'none',
              fontSize: 'var(--text-sm)',
            }}
          >
            升级到 Pro
          </a>
        )}
      </header>

      {data.plugins.length === 0 ? (
        <div style={{ color: 'var(--color-text-secondary)' }}>
          暂无可用插件。可在 <strong>设置 → PluginHub</strong> 配置 hub URL + license key 接入真实市场。
        </div>
      ) : (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(360px, 1fr))', gap: 'var(--space-4)' }}>
          {data.plugins.map((p) => (
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
                    ? `${PLAN_LABELS[p.min_plan] ?? p.min_plan} ✓`
                    : `需 ${PLAN_LABELS[p.min_plan] ?? p.min_plan}`}
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
                    {installing === p.id ? '安装中...' : '安装'}
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
                    {installing === p.id ? '启动中...' : `免费试用 ${p.trial_days} 天`}
                  </button>
                ) : (
                  <a
                    href={data.upgrade_url}
                    target="_blank"
                    rel="noopener"
                    style={{
                      flex: 1,
                      padding: 'var(--space-2)',
                      background: 'var(--color-bg)',
                      color: 'var(--color-text)',
                      border: '1px solid var(--color-border)',
                      borderRadius: 'var(--radius-sm)',
                      textDecoration: 'none',
                      textAlign: 'center',
                    }}
                  >
                    需升级会员
                  </a>
                )}
              </div>
            </article>
          ))}
        </div>
      )}
    </div>
  );
}
