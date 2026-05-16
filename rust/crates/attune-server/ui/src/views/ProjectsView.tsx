/** Projects 视图 · Sprint 1 Phase D-1
 *
 * 通用 Project 卷宗管理（不带行业字眼，kind 是自由字符串由 plugin 定义）：
 *   - 列出所有 Project（title + kind + 创建/更新时间）
 *   - 新建项目（modal: title + kind 输入）
 *   - 选中后右侧展示 files + timeline
 *
 * attune-core 边界：UI 不约束 kind 取值，纯路由透传。
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState, Modal, PluginForm, toast } from '../components';
import { api } from '../store/api';
import { t } from '../i18n';

// ── 类型（与后端 routes/projects.rs ProjectListResponse 等对齐） ─────────────
interface Project {
  id: string;
  title: string;
  kind: string;
  metadata_encrypted: number[] | null;
  created_at: number; // 秒
  updated_at: number; // 秒
  archived: boolean;
}

interface ProjectFile {
  project_id: string;
  file_id: string;
  role: string;
  added_at: number; // 秒
}

interface TimelineEntry {
  project_id: string;
  ts_ms: number; // 毫秒
  event_type: string;
  payload_encrypted: number[] | null;
}

interface ProjectListResponse {
  projects: Project[];
  total: number;
}

interface FilesListResponse {
  files: ProjectFile[];
}

interface TimelineResponse {
  entries: TimelineEntry[];
}

// ── plugin-form 发现（/api/v1/plugins） ──────────────────────────────────────
interface PluginAgent {
  id: string;
  description: string;
  case_kinds?: string[];
}
interface PluginUiComponent {
  id: string;
  target: string;
  description: string;
}
interface PluginInfo {
  id: string;
  agents?: PluginAgent[];
  ui_components?: PluginUiComponent[];
}
interface PluginListResp {
  plugins: PluginInfo[];
}
/** 匹配到某 project kind 的 plugin 表单引用 */
interface FormRef {
  pluginId: string;
  formId: string;
  label: string;
}

/** 列出 ui_component 的 target agent 的 case_kinds 含 `kind` 的表单。 */
function matchedForms(plugins: PluginInfo[], kind: string): FormRef[] {
  const out: FormRef[] = [];
  for (const p of plugins) {
    const agents = p.agents ?? [];
    for (const c of p.ui_components ?? []) {
      const agentId = c.target.startsWith('agent:') ? c.target.slice('agent:'.length) : c.target;
      const agent = agents.find((a) => a.id === agentId);
      if (agent && (agent.case_kinds ?? []).includes(kind)) {
        out.push({ pluginId: p.id, formId: c.id, label: c.description || agent.description || c.id });
      }
    }
  }
  return out;
}

// ── 主视图 ──────────────────────────────────────────────────────────────────
export function ProjectsView(): JSX.Element {
  const projects = useSignal<Project[]>([]);
  const loading = useSignal(false);
  const error = useSignal<string | null>(null);
  const showCreate = useSignal(false);
  const newTitle = useSignal('');
  const newKind = useSignal('generic');
  const selectedId = useSignal<string | null>(null);
  const files = useSignal<ProjectFile[]>([]);
  const timeline = useSignal<TimelineEntry[]>([]);
  const plugins = useSignal<PluginInfo[]>([]);
  // 当前打开的 plugin 表单（modal）
  const openForm = useSignal<FormRef | null>(null);

  const reload = async (): Promise<void> => {
    loading.value = true;
    error.value = null;
    try {
      const res = await api.get<ProjectListResponse>('/projects');
      projects.value = res.projects;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  };

  useEffect(() => {
    void reload();
    // plugin 列表（含 agents.case_kinds + ui_components）—— 用于按项目类型发现可用表单
    void (async () => {
      try {
        const res = await api.get<PluginListResp>('/plugins');
        plugins.value = res.plugins ?? [];
      } catch {
        plugins.value = [];
      }
    })();
  }, []);

  const onCreate = async (): Promise<void> => {
    const title = newTitle.value.trim();
    const kind = newKind.value.trim() || 'generic';
    if (!title) {
      toast('error', t('projects.toast.title_required'));
      return;
    }
    try {
      await api.post('/projects', { title, kind });
      newTitle.value = '';
      newKind.value = 'generic';
      showCreate.value = false;
      toast('success', t('projects.toast.created'));
      await reload();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      error.value = msg;
      toast('error', t('projects.toast.create_failed', { message: msg }));
    }
  };

  const onSelect = async (id: string): Promise<void> => {
    selectedId.value = id;
    files.value = [];
    timeline.value = [];
    try {
      const [f, t] = await Promise.all([
        api.get<FilesListResponse>(`/projects/${id}/files`),
        api.get<TimelineResponse>(`/projects/${id}/timeline`),
      ]);
      files.value = f.files;
      timeline.value = t.entries;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  };

  // 空态
  if (!loading.value && projects.value.length === 0 && !error.value) {
    return (
      <div
        style={{
          padding: 'var(--space-5)',
          height: '100%',
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-4)',
        }}
      >
        <ProjectsHeader
          onCreate={() => (showCreate.value = true)}
          onReload={() => void reload()}
          loading={loading.value}
        />
        <EmptyState
          icon="🗂"
          title={t('projects.empty.title')}
          description={t('projects.empty.desc')}
          actions={[
            {
              label: t('projects.empty.action'),
              onClick: () => (showCreate.value = true),
              variant: 'primary',
            },
          ]}
        />
        {showCreate.value && (
          <CreateProjectModal
            title={newTitle}
            kind={newKind}
            onCancel={() => (showCreate.value = false)}
            onConfirm={() => void onCreate()}
          />
        )}
      </div>
    );
  }

  const selProject = projects.value.find((p) => p.id === selectedId.value) ?? null;
  const projForms = selProject ? matchedForms(plugins.value, selProject.kind) : [];

  return (
    <div
      style={{
        padding: 'var(--space-5)',
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-4)',
      }}
    >
      <ProjectsHeader
        onCreate={() => (showCreate.value = true)}
        onReload={() => void reload()}
        loading={loading.value}
      />

      {error.value && (
        <div
          role="alert"
          style={{
            padding: 'var(--space-2) var(--space-3)',
            background: 'var(--color-error-bg, #ffe6e6)',
            color: 'var(--color-error, #c00)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-md)',
            fontSize: 'var(--text-sm)',
          }}
        >
          ⚠ {error.value}
        </div>
      )}

      <div
        style={{
          flex: 1,
          display: 'grid',
          gridTemplateColumns: '320px 1fr',
          gap: 'var(--space-4)',
          overflow: 'hidden',
          minHeight: 0,
        }}
      >
        {/* 左：列表 */}
        <aside
          style={{
            overflow: 'auto',
            borderRight: '1px solid var(--color-border)',
            paddingRight: 'var(--space-3)',
            display: 'flex',
            flexDirection: 'column',
            gap: 'var(--space-2)',
          }}
        >
          {projects.value.map((p) => (
            <ProjectRow
              key={p.id}
              project={p}
              active={selectedId.value === p.id}
              onClick={() => void onSelect(p.id)}
            />
          ))}
        </aside>

        {/* 右：详情 */}
        <section style={{ overflow: 'auto' }}>
          {selectedId.value === null ? (
            <div
              style={{
                padding: 'var(--space-6)',
                textAlign: 'center',
                color: 'var(--color-text-secondary)',
              }}
            >
              {t('projects.select_hint')}
            </div>
          ) : (
            <ProjectDetail
              files={files.value}
              timeline={timeline.value}
              forms={projForms}
              onRunForm={(f) => (openForm.value = f)}
            />
          )}
        </section>
      </div>

      {showCreate.value && (
        <CreateProjectModal
          title={newTitle}
          kind={newKind}
          onCancel={() => (showCreate.value = false)}
          onConfirm={() => void onCreate()}
        />
      )}

      {openForm.value && (
        <Modal open onClose={() => (openForm.value = null)} title={openForm.value.label}>
          <PluginForm
            pluginId={openForm.value.pluginId}
            formId={openForm.value.formId}
            onClose={() => (openForm.value = null)}
          />
        </Modal>
      )}
    </div>
  );
}

// ── 子组件 ──────────────────────────────────────────────────────────────────

function ProjectsHeader({
  onCreate,
  onReload,
  loading,
}: {
  onCreate: () => void;
  onReload: () => void;
  loading: boolean;
}): JSX.Element {
  return (
    <header
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
      }}
    >
      <h2 style={{ fontSize: 'var(--text-xl)', fontWeight: 600, margin: 0 }}>
        {t('projects.title')}
      </h2>
      <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
        <Button variant="primary" size="sm" onClick={onCreate}>
          {t('projects.create')}
        </Button>
        <Button variant="secondary" size="sm" onClick={onReload} disabled={loading}>
          {loading ? t('common.loading') : t('items.refresh')}
        </Button>
      </div>
    </header>
  );
}

function ProjectRow({
  project: p,
  active,
  onClick,
}: {
  project: Project;
  active: boolean;
  onClick: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      className="interactive"
      aria-current={active ? 'true' : undefined}
      style={{
        textAlign: 'left',
        padding: 'var(--space-3) var(--space-4)',
        background: active ? 'var(--color-surface-hover)' : 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderLeft: active ? '2px solid var(--color-accent)' : '2px solid transparent',
        borderRadius: 'var(--radius-md)',
        cursor: 'pointer',
        display: 'flex',
        flexDirection: 'column',
        gap: 4,
      }}
    >
      <div
        style={{
          fontSize: 'var(--text-base)',
          color: 'var(--color-text)',
          fontWeight: 500,
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
        }}
      >
        {p.title || t('projects.untitled')}
      </div>
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          gap: 'var(--space-2)',
          fontSize: 'var(--text-xs)',
          color: 'var(--color-text-secondary)',
        }}
      >
        <span
          style={{
            padding: '1px 6px',
            background: 'var(--color-bg)',
            border: '1px solid var(--color-border)',
            borderRadius: 'var(--radius-sm)',
            fontFamily: 'var(--font-mono)',
          }}
        >
          {p.kind}
        </span>
        <time dateTime={new Date(p.updated_at * 1000).toISOString()}>
          {fmtSecs(p.updated_at)}
        </time>
      </div>
    </button>
  );
}

function ProjectDetail({
  files,
  timeline,
  forms,
  onRunForm,
}: {
  files: ProjectFile[];
  timeline: TimelineEntry[];
  forms: FormRef[];
  onRunForm: (f: FormRef) => void;
}): JSX.Element {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-4)',
      }}
    >
      {forms.length > 0 && (
        <section>
          <h3
            style={{
              fontSize: 'var(--text-base)',
              fontWeight: 600,
              margin: '0 0 var(--space-2) 0',
              color: 'var(--color-text)',
            }}
          >
            {t('projects.agents.title')}
          </h3>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-2)' }}>
            {forms.map((f) => (
              <div
                key={`${f.pluginId}/${f.formId}`}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  gap: 'var(--space-3)',
                  padding: 'var(--space-2) var(--space-3)',
                  background: 'var(--color-surface)',
                  border: '1px solid var(--color-border)',
                  borderRadius: 'var(--radius-md)',
                }}
              >
                <span style={{ fontSize: 'var(--text-sm)' }}>{f.label}</span>
                <Button variant="primary" size="sm" onClick={() => onRunForm(f)}>
                  ▶ {t('projects.agents.run')}
                </Button>
              </div>
            ))}
          </div>
        </section>
      )}
      <section>
        <h3
          style={{
            fontSize: 'var(--text-base)',
            fontWeight: 600,
            margin: '0 0 var(--space-2) 0',
            color: 'var(--color-text)',
          }}
        >
          {t('projects.files.heading', { count: files.length })}
        </h3>
        {files.length === 0 ? (
          <div
            style={{
              padding: 'var(--space-3)',
              color: 'var(--color-text-secondary)',
              fontSize: 'var(--text-sm)',
            }}
          >
            {t('projects.files.empty')}
          </div>
        ) : (
          <table
            style={{
              width: '100%',
              borderCollapse: 'collapse',
              fontSize: 'var(--text-sm)',
            }}
          >
            <thead>
              <tr>
                <th style={th}>{t('projects.files.col_file')}</th>
                <th style={th}>{t('projects.files.col_role')}</th>
                <th style={th}>{t('projects.files.col_added')}</th>
              </tr>
            </thead>
            <tbody>
              {files.map((f) => (
                <tr key={f.file_id}>
                  <td style={td}>
                    <code style={{ fontFamily: 'var(--font-mono)' }}>{f.file_id}</code>
                  </td>
                  <td style={td}>{f.role || '—'}</td>
                  <td style={td}>{fmtSecs(f.added_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section>
        <h3
          style={{
            fontSize: 'var(--text-base)',
            fontWeight: 600,
            margin: '0 0 var(--space-2) 0',
            color: 'var(--color-text)',
          }}
        >
          {t('projects.timeline.heading', { count: timeline.length })}
        </h3>
        {timeline.length === 0 ? (
          <div
            style={{
              padding: 'var(--space-3)',
              color: 'var(--color-text-secondary)',
              fontSize: 'var(--text-sm)',
            }}
          >
            {t('projects.timeline.empty')}
          </div>
        ) : (
          <ul
            style={{
              listStyle: 'none',
              padding: 0,
              margin: 0,
              display: 'flex',
              flexDirection: 'column',
              gap: 0,
            }}
          >
            {timeline.map((t, i) => (
              <li
                key={i}
                style={{
                  display: 'flex',
                  gap: 'var(--space-3)',
                  padding: 'var(--space-2) 0',
                  borderBottom: '1px dotted var(--color-border)',
                  fontSize: 'var(--text-sm)',
                }}
              >
                <time
                  dateTime={new Date(t.ts_ms).toISOString()}
                  style={{
                    color: 'var(--color-text-secondary)',
                    whiteSpace: 'nowrap',
                    fontFamily: 'var(--font-mono)',
                    fontSize: 'var(--text-xs)',
                  }}
                >
                  {fmtMs(t.ts_ms)}
                </time>
                <span style={{ color: 'var(--color-text)' }}>{t.event_type}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function CreateProjectModal({
  title,
  kind,
  onCancel,
  onConfirm,
}: {
  title: { value: string };
  kind: { value: string };
  onCancel: () => void;
  onConfirm: () => void;
}): JSX.Element {
  return (
    <Modal open onClose={onCancel} title={t('projects.create.modal_title')}>
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-3)',
          minWidth: 360,
        }}
      >
        <label style={labelStyle}>
          <span>{t('projects.field.name')}</span>
          <input
            type="text"
            value={title.value}
            onInput={(e) => (title.value = (e.currentTarget as HTMLInputElement).value)}
            placeholder={t('projects.field.name_placeholder')}
            autoFocus
            style={inputStyle}
          />
        </label>
        <label style={labelStyle}>
          <span>{t('projects.field.kind')}</span>
          <input
            type="text"
            value={kind.value}
            onInput={(e) => (kind.value = (e.currentTarget as HTMLInputElement).value)}
            placeholder={t('projects.field.kind_placeholder')}
            style={inputStyle}
          />
        </label>
        <div
          style={{
            display: 'flex',
            justifyContent: 'flex-end',
            gap: 'var(--space-2)',
            marginTop: 'var(--space-2)',
          }}
        >
          <Button variant="secondary" size="sm" onClick={onCancel}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" size="sm" onClick={onConfirm}>
            {t('projects.empty.action')}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

// ── 样式 / 工具 ────────────────────────────────────────────────────────────
const th = {
  textAlign: 'left' as const,
  padding: 'var(--space-2) var(--space-3)',
  borderBottom: '1px solid var(--color-border)',
  fontWeight: 600,
  color: 'var(--color-text-secondary)',
  fontSize: 'var(--text-xs)',
};

const td = {
  padding: 'var(--space-2) var(--space-3)',
  borderBottom: '1px solid var(--color-border)',
  color: 'var(--color-text)',
};

const labelStyle = {
  display: 'flex',
  flexDirection: 'column' as const,
  gap: 4,
  fontSize: 'var(--text-sm)',
  color: 'var(--color-text-secondary)',
};

const inputStyle = {
  padding: 'var(--space-2) var(--space-3)',
  fontSize: 'var(--text-sm)',
  background: 'var(--color-bg)',
  border: '1px solid var(--color-border)',
  borderRadius: 'var(--radius-md)',
  color: 'var(--color-text)',
  outline: 'none',
};

function fmtSecs(unixSec: number): string {
  if (!unixSec) return '—';
  return fmtDate(new Date(unixSec * 1000));
}

function fmtMs(unixMs: number): string {
  if (!unixMs) return '—';
  return fmtDate(new Date(unixMs));
}

function fmtDate(d: Date): string {
  try {
    const now = Date.now();
    const diff = now - d.getTime();
    if (diff < 60_000) return t('projects.time.just_now');
    if (diff < 86_400_000) {
      const h = d.getHours().toString().padStart(2, '0');
      const m = d.getMinutes().toString().padStart(2, '0');
      return t('projects.time.today', { time: `${h}:${m}` });
    }
    if (diff < 2 * 86_400_000) return t('projects.time.yesterday');
    if (diff < 7 * 86_400_000) {
      return t('projects.time.days_ago', { days: Math.floor(diff / 86_400_000) });
    }
    return d.toLocaleDateString();
  } catch {
    return d.toISOString().slice(0, 10);
  }
}
