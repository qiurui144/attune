/** Wizard Step 5 · 第一口知识 · 三选一 */

import type { JSX } from 'preact';
import { useState, useRef } from 'preact/hooks';
import { Button, Input, Tooltip } from '../components';
import { toast } from '../components/Toast';
import { t } from '../i18n';
import { api } from '../store/api';
import type { WizardContext } from './types';

type DataMode = 'folder' | 'import' | 'skip';

export type Step5Props = {
  ctx: WizardContext;
  onUpdate: (partial: Partial<WizardContext>) => void;
  onFinish: () => void;
};

export function Step5Data({ ctx, onUpdate, onFinish }: Step5Props): JSX.Element {
  const [mode, setMode] = useState<DataMode | null>(ctx.dataMode);
  const [folderPaths, setFolderPaths] = useState<string[]>(ctx.boundFolders ?? []);
  const [folderPicking, setFolderPicking] = useState(false);
  const [manualPath, setManualPath] = useState('');
  const [importing, setImporting] = useState(false);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  // Tauri 桌面壳里才有原生目录选择器;headless/浏览器(K3 一体机纯 Web)回退到手填
  // 绝对路径(与 RemoteView LocalForm / SettingsView 文件夹管理一致)。两条路径都
  // push 到同一个 folderPaths,统一走 /index/bind,headless 首次开箱不再被卡死。
  const canPickFolder = typeof window !== 'undefined'
    && Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);

  function addFolderPath(raw: string): boolean {
    const path = raw.trim();
    if (!path) return false;
    let added = false;
    setFolderPaths((current) => {
      if (current.includes(path)) return current;
      added = true;
      return [...current, path];
    });
    return added;
  }

  function submitManualPath() {
    if (addFolderPath(manualPath)) {
      setManualPath('');
    }
  }

  async function pickFolder() {
    if (!canPickFolder) {
      // headless/浏览器无原生选择器 → 引导用户使用下方手填路径框,不再死路一条。
      toast('info', t('wizard.data.folder.toast_manual_hint'));
      return;
    }

    setFolderPicking(true);
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({
        directory: true,
        multiple: true,
        title: t('wizard.data.folder.dialog_title'),
      });
      const chosen = Array.isArray(selected) ? selected : selected ? [selected] : [];
      for (const path of chosen) {
        addFolderPath(path);
      }
    } catch (e) {
      toast('error', e instanceof Error ? e.message : String(e));
    } finally {
      setFolderPicking(false);
    }
  }

  async function handleFinish() {
    if (!mode) {
      toast('warning', t('wizard.data.toast.choose_mode'));
      return;
    }
    if (mode === 'folder' && folderPaths.length === 0) {
      toast('warning', t('wizard.data.toast.need_folder'));
      return;
    }
    onUpdate({ dataMode: mode });
    setImporting(true);

    try {
      if (mode === 'folder' && folderPaths.length > 0) {
        await Promise.all(folderPaths.map((path) => api.post('/index/bind', { path, recursive: true })));
        onUpdate({ boundFolders: folderPaths });
        toast('success', t('wizard.data.toast.bound_n', { count: folderPaths.length }));
      } else if (mode === 'import') {
        const file = fileInputRef.current?.files?.[0];
        if (file) {
          // Critical 1.3 修复：文件大小 + shape 校验，防恶意 profile 打挂后端
          const MAX_SIZE = 50 * 1024 * 1024; // 50 MB
          if (file.size > MAX_SIZE) {
            throw new Error(t('wizard.data.err.file_too_large', { mb: (MAX_SIZE / 1024 / 1024).toFixed(0) }));
          }
          const text = await file.text();
          let profile: unknown;
          try {
            profile = JSON.parse(text);
          } catch {
            throw new Error(t('wizard.data.err.invalid_json'));
          }
          if (
            !profile ||
            typeof profile !== 'object' ||
            Array.isArray(profile) ||
            !('version' in (profile as object))
          ) {
            throw new Error(t('wizard.data.err.invalid_profile'));
          }
          await api.post('/profile/import', profile);
          onUpdate({ importedProfile: file.name });
          toast('success', t('wizard.data.toast.imported', { name: file.name }));
        }
      }
      onFinish();
    } catch (e) {
      toast('error', e instanceof Error ? e.message : String(e));
      setImporting(false);
    }
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-5)' }}>
      <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0, display: 'flex', alignItems: 'center' }}>
        {t('wizard.data.heading')}
        <Tooltip text={t('wizard.help.data_bind_folder')} />
      </h2>

      <div
        style={{
          display: 'grid',
          gridTemplateColumns: '1fr 1fr 1fr',
          gap: 'var(--space-3)',
        }}
      >
        {/* 绑定文件夹 */}
        <Option
          icon="📂"
          title={t('wizard.data.folder.title')}
          desc={canPickFolder ? t('wizard.data.folder.desc') : t('wizard.data.folder.desc_manual')}
          selected={mode === 'folder'}
          onClick={() => setMode('folder')}
        >
          {mode === 'folder' && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
              {canPickFolder ? (
                <div
                  role="button"
                  tabIndex={0}
                  aria-disabled={folderPicking}
                  onClick={(e) => {
                    e.stopPropagation();
                    void pickFolder();
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault();
                      e.stopPropagation();
                      void pickFolder();
                    }
                  }}
                  style={{
                    display: 'inline-flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    minHeight: 36,
                    padding: '0 var(--space-3)',
                    borderRadius: 'var(--radius-sm)',
                    border: '1px solid var(--color-border)',
                    background: folderPicking ? 'var(--color-surface-muted)' : 'var(--color-surface)',
                    color: folderPicking ? 'var(--color-text-secondary)' : 'var(--color-text)',
                    cursor: folderPicking ? 'not-allowed' : 'pointer',
                    userSelect: 'none',
                    fontSize: 'var(--text-xs)',
                    fontWeight: 600,
                  }}
                >
                  {folderPicking ? t('wizard.data.folder.btn_picking') : t('wizard.data.folder.btn_add')}
                </div>
              ) : (
                // headless/纯 Web:手填绝对路径 + 添加按钮(无原生选择器时的等价入口)
                <div
                  style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'flex-end' }}
                  onClick={(e) => e.stopPropagation()}
                >
                  <div style={{ flex: 1 }}>
                    <Input
                      label={t('wizard.data.folder.manual_label')}
                      value={manualPath}
                      onInput={(e) => setManualPath(e.currentTarget.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') {
                          e.preventDefault();
                          submitManualPath();
                        }
                      }}
                      placeholder={t('wizard.data.folder.manual_placeholder')}
                    />
                  </div>
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={!manualPath.trim()}
                    onClick={(e) => {
                      e.stopPropagation();
                      submitManualPath();
                    }}
                  >
                    {t('wizard.data.folder.btn_add')}
                  </Button>
                </div>
              )}
              <div
                style={{
                  minHeight: 56,
                  padding: 'var(--space-2)',
                  fontSize: 'var(--text-xs)',
                  border: '1px solid var(--color-border)',
                  borderRadius: 'var(--radius-sm)',
                  background: 'var(--color-surface-muted)',
                  color: folderPaths.length ? 'var(--color-text)' : 'var(--color-text-secondary)',
                }}
                onClick={(e) => {
                  e.stopPropagation();
                }}
              >
                {folderPaths.length > 0 ? (
                  <div style={{ display: 'flex', flexWrap: 'wrap', gap: 'var(--space-2)' }}>
                    {folderPaths.map((path) => (
                      <FolderChip
                        key={path}
                        path={path}
                        onRemove={() => {
                          setFolderPaths((current) => current.filter((item) => item !== path));
                        }}
                      />
                    ))}
                  </div>
                ) : (
                  t('wizard.data.folder.empty')
                )}
              </div>
            </div>
          )}
        </Option>

        {/* 导入 profile */}
        <Option
          icon="📥"
          title={t('wizard.data.import.title')}
          desc={t('wizard.data.import.desc')}
          selected={mode === 'import'}
          onClick={() => {
            setMode('import');
            fileInputRef.current?.click();
          }}
        >
          <>
            <input
              ref={fileInputRef}
              type="file"
              accept=".json,.vault-profile"
              style={{ display: 'none' }}
              onClick={(e) => e.stopPropagation()}
            />
            {mode === 'import' && fileInputRef.current?.files?.[0] && (
              <div
                style={{
                  marginTop: 'var(--space-2)',
                  fontSize: 'var(--text-xs)',
                  color: 'var(--color-accent)',
                }}
              >
                ✓ {fileInputRef.current.files[0].name}
              </div>
            )}
          </>
        </Option>

        {/* 跳过 */}
        <Option
          icon="→"
          title={t('wizard.data.skip.title')}
          desc={t('wizard.data.skip.desc')}
          selected={mode === 'skip'}
          onClick={() => setMode('skip')}
        />
      </div>

      <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
        <Button
          variant="primary"
          size="lg"
          loading={importing}
          disabled={!mode}
          onClick={handleFinish}
        >
          {t('wizard.data.finish')} →
        </Button>
      </div>
    </div>
  );
}

function Option({
  icon,
  title,
  desc,
  selected,
  onClick,
  children,
}: {
  icon: string;
  title: string;
  desc: string;
  selected: boolean;
  onClick: () => void;
  children?: JSX.Element | JSX.Element[] | false | null;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={selected}
      className="interactive"
      style={{
        padding: 'var(--space-4)',
        background: 'var(--color-surface)',
        border: `2px solid ${selected ? 'var(--color-accent)' : 'var(--color-border)'}`,
        borderRadius: 'var(--radius-lg)',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-2)',
        textAlign: 'left',
        cursor: 'pointer',
        minHeight: 160,
      }}
    >
      <div style={{ fontSize: 24 }} aria-hidden="true">
        {icon}
      </div>
      <h3 style={{ fontSize: 'var(--text-base)', fontWeight: 600, margin: 0 }}>
        {title}
      </h3>
      <p
        style={{
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-secondary)',
          margin: 0,
          lineHeight: 1.5,
        }}
      >
        {desc}
      </p>
      {children}
    </button>
  );
}

function FolderChip({ path, onRemove }: { path: string; onRemove: () => void }): JSX.Element {
  return (
    <div
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 'var(--space-2)',
        maxWidth: '100%',
        padding: '6px 10px',
        borderRadius: '999px',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
      }}
    >
      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
        {path}
      </span>
      <span
        role="button"
        tabIndex={0}
        aria-label={t('wizard.data.folder.aria_remove', { path })}
        onClick={(e) => {
          e.stopPropagation();
          onRemove();
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            e.stopPropagation();
            onRemove();
          }
        }}
        style={{
          border: 0,
          background: 'transparent',
          color: 'var(--color-text-secondary)',
          cursor: 'pointer',
          padding: 0,
          lineHeight: 1,
          fontSize: 'var(--text-sm)',
        }}
      >
        ×
      </span>
    </div>
  );
}
