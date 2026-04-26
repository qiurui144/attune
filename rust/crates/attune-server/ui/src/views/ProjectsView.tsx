/** Projects 视图 · Sprint 1 Phase D-1
 *
 * 通用 Project 卷宗管理（不带行业字眼，kind 是自由字符串由 plugin 定义）：
 *   - 列出所有 Project（title + kind + 创建/更新时间）
 *   - 新建 Project（modal: title + kind 输入）
 *   - 选中后右侧展示 files + timeline
 *
 * attune-core 边界：UI 不约束 kind 取值，纯路由透传。
 */

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { Button, EmptyState, Modal, toast } from '../components';
import { api } from '../store/api';

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
  }, []);

  const onCreate = async (): Promise<void> => {
    const title = newTitle.value.trim();
    const kind = newKind.value.trim() || 'generic';
    if (!title) {
      toast('error', 'Title 必填');
      return;
    }
    try {
      await api.post('/projects', { title, kind });
      newTitle.value = '';
      newKind.value = 'generic';
      showCreate.value = false;
      toast('success', '已创建');
      await reload();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      error.value = msg;
      toast('error', `创建失败：${msg}`);
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
          title="暂无 Project"
          description="点击「新建 Project」开始整理资料、证据与思路。"
          actions={[
            {
              label: '新建 Project',
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
              选择左侧 Project 查看详情
            </div>
          ) : (
            <ProjectDetail files={files.value} timeline={timeline.value} />
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
        🗂 Projects
      </h2>
      <div style={{ display: 'flex', gap: 'var(--space-2)' }}>
        <Button variant="primary" size="sm" onClick={onCreate}>
          + 新建 Project
        </Button>
        <Button variant="secondary" size="sm" onClick={onReload} disabled={loading}>
          {loading ? '加载中…' : '⟳ 刷新'}
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
        {p.title || '(无标题)'}
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
}: {
  files: ProjectFile[];
  timeline: TimelineEntry[];
}): JSX.Element {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-4)',
      }}
    >
      <section>
        <h3
          style={{
            fontSize: 'var(--text-base)',
            fontWeight: 600,
            margin: '0 0 var(--space-2) 0',
            color: 'var(--color-text)',
          }}
        >
          Files ({files.length})
        </h3>
        {files.length === 0 ? (
          <div
            style={{
              padding: 'var(--space-3)',
              color: 'var(--color-text-secondary)',
              fontSize: 'var(--text-sm)',
            }}
          >
            无文件归档
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
                <th style={th}>File ID</th>
                <th style={th}>Role</th>
                <th style={th}>Added</th>
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
          Timeline ({timeline.length})
        </h3>
        {timeline.length === 0 ? (
          <div
            style={{
              padding: 'var(--space-3)',
              color: 'var(--color-text-secondary)',
              fontSize: 'var(--text-sm)',
            }}
          >
            暂无事件
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
    <Modal open onClose={onCancel} title="新建 Project">
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 'var(--space-3)',
          minWidth: 360,
        }}
      >
        <label style={labelStyle}>
          <span>Title</span>
          <input
            type="text"
            value={title.value}
            onInput={(e) => (title.value = (e.currentTarget as HTMLInputElement).value)}
            placeholder="如：北京客户A / 论文研究 X / 个人项目"
            autoFocus
            style={inputStyle}
          />
        </label>
        <label style={labelStyle}>
          <span>Kind（自由字符串，由 plugin 定义；默认 generic）</span>
          <input
            type="text"
            value={kind.value}
            onInput={(e) => (kind.value = (e.currentTarget as HTMLInputElement).value)}
            placeholder="generic / deal / topic / ..."
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
            取消
          </Button>
          <Button variant="primary" size="sm" onClick={onConfirm}>
            创建
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
    if (diff < 60_000) return '刚刚';
    if (diff < 86_400_000) {
      const h = d.getHours().toString().padStart(2, '0');
      const m = d.getMinutes().toString().padStart(2, '0');
      return `今天 ${h}:${m}`;
    }
    if (diff < 2 * 86_400_000) return '昨天';
    if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} 天前`;
    return d.toLocaleDateString();
  } catch {
    return d.toISOString().slice(0, 10);
  }
}
