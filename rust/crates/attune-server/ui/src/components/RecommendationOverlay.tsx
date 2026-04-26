/** Sprint 1 Phase D-2: ws 推送的项目推荐 · 右下角浮窗
 *
 * 展示两类 trigger：
 *   - file_uploaded：列出候选 project，每条带"归档"按钮 → POST /projects/:id/files
 *   - chat_keyword：纯文本提示
 *
 * 通用 UI（无行业字眼）。Toast 已占据右下角更下方，本组件 z-index 略高于 toast。
 */

import type { JSX } from 'preact';
import { Button } from './Button';
import { toast } from './Toast';
import { api } from '../store/api';
import {
  recommendations,
  dismissRecommendation,
} from '../store/signals';
import type { RecommendationPayload } from '../store/signals';

async function acceptRecommendation(
  payload: Extract<RecommendationPayload, { trigger: 'file_uploaded' }>,
  candidateIdx: number,
): Promise<void> {
  const cand = payload.candidates[candidateIdx];
  if (!cand) return;
  // role 留空：后端允许，由用户后续在 ProjectsView 设置具体角色
  await api.post(`/projects/${cand.project_id}/files`, {
    file_id: payload.file_id,
    role: '',
  });
}

export function RecommendationOverlay(): JSX.Element | null {
  if (recommendations.value.length === 0) return null;

  return (
    <div
      aria-live="polite"
      style={{
        position: 'fixed',
        bottom: 'calc(var(--space-5) + 80px)', // 让位 ToastContainer
        right: 'var(--space-5)',
        display: 'flex',
        flexDirection: 'column',
        gap: 'var(--space-2)',
        zIndex: 1900,
        maxWidth: 360,
        pointerEvents: 'none',
      }}
    >
      {recommendations.value.map((rec, i) => (
        <RecommendationCard key={i} rec={rec} index={i} />
      ))}
    </div>
  );
}

function RecommendationCard({
  rec,
  index,
}: {
  rec: RecommendationPayload;
  index: number;
}): JSX.Element {
  return (
    <div
      role="status"
      className="fade-slide-in"
      style={{
        position: 'relative',
        background: 'var(--color-surface)',
        border: '1px solid var(--color-border)',
        borderRadius: 'var(--radius-md)',
        padding: 'var(--space-3) var(--space-4)',
        boxShadow: 'var(--shadow-lg)',
        fontSize: 'var(--text-sm)',
        color: 'var(--color-text)',
        pointerEvents: 'auto',
      }}
    >
      <button
        type="button"
        onClick={() => dismissRecommendation(index)}
        aria-label="忽略"
        style={{
          position: 'absolute',
          top: 'var(--space-2)',
          right: 'var(--space-2)',
          background: 'transparent',
          border: 'none',
          color: 'var(--color-text-secondary)',
          cursor: 'pointer',
          fontSize: 'var(--text-lg)',
          padding: 0,
          lineHeight: 1,
        }}
      >
        ×
      </button>

      {rec.trigger === 'file_uploaded' && (
        <FileUploadedCard rec={rec} index={index} />
      )}
      {rec.trigger === 'chat_keyword' && (
        <ChatKeywordCard rec={rec} />
      )}
    </div>
  );
}

function FileUploadedCard({
  rec,
  index,
}: {
  rec: Extract<RecommendationPayload, { trigger: 'file_uploaded' }>;
  index: number;
}): JSX.Element {
  return (
    <div>
      <div
        style={{
          fontWeight: 500,
          marginBottom: 'var(--space-2)',
          paddingRight: 'var(--space-4)',
        }}
      >
        新文件可归到已有集合
      </div>
      <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
        {rec.candidates.map((c, ci) => (
          <li
            key={ci}
            style={{
              padding: 'var(--space-2) 0',
              borderBottom:
                ci < rec.candidates.length - 1
                  ? '1px dotted var(--color-border)'
                  : 'none',
            }}
          >
            <div style={{ fontWeight: 500 }}>{c.project_title}</div>
            <div
              style={{
                fontSize: 'var(--text-xs)',
                color: 'var(--color-text-secondary)',
                marginBottom: 'var(--space-1)',
              }}
            >
              相似度 {(c.score * 100).toFixed(0)}%
              {c.overlapping_entities.length > 0 && (
                <span style={{ marginLeft: 'var(--space-2)' }}>
                  · {c.overlapping_entities.slice(0, 2).join(' · ')}
                </span>
              )}
            </div>
            <Button
              variant="primary"
              size="sm"
              onClick={async () => {
                try {
                  await acceptRecommendation(rec, ci);
                  toast('success', `已归档到「${c.project_title}」`);
                  dismissRecommendation(index);
                } catch (e) {
                  const msg = e instanceof Error ? e.message : String(e);
                  toast('error', `归档失败：${msg}`);
                }
              }}
            >
              归档到此集合
            </Button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function ChatKeywordCard({
  rec,
}: {
  rec: Extract<RecommendationPayload, { trigger: 'chat_keyword' }>;
}): JSX.Element {
  return (
    <div>
      <div
        style={{
          fontWeight: 500,
          marginBottom: 'var(--space-2)',
          paddingRight: 'var(--space-4)',
        }}
      >
        关键词提示
      </div>
      <div
        style={{
          fontSize: 'var(--text-sm)',
          lineHeight: 1.4,
          color: 'var(--color-text-secondary)',
        }}
      >
        {rec.suggestion ?? '消息中包含可关联到集合的关键词'}
      </div>
      {rec.matched_keywords && rec.matched_keywords.length > 0 && (
        <div
          style={{
            fontSize: 'var(--text-xs)',
            color: 'var(--color-text-secondary)',
            marginTop: 'var(--space-2)',
          }}
        >
          匹配：{rec.matched_keywords.join('、')}
        </div>
      )}
    </div>
  );
}
