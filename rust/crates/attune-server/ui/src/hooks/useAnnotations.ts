/** useAnnotations · 批注 CRUD + AI 4 角度
 *
 * 字段名与服务端 `routes/annotations.rs` 契约严格对齐：
 * offset_start / offset_end / text_snippet / label / content —— 不再用前端私名，
 * 避免读写双向错位（曾导致 AI 批注高亮全文 + 面板空 + 手动批注「添加失败」）。
 */
import { api } from '../store/api';

export type Annotation = {
  id: string;
  item_id: string;
  offset_start: number;
  offset_end: number;
  text_snippet: string;
  /** 标签：疑问/灵感/风险/重点/笔记（用户）或 AI 角度标签 */
  label: string;
  color?: string;
  /** 批注正文：用户备注 或 AI 分析结论 */
  content?: string;
  source: 'user' | 'ai';
  created_at: string;
};

export const PRESET_TAGS = [
  { key: '疑问', emoji: '❓', color: '#D4A574' },
  { key: '灵感', emoji: '💡', color: '#E7C87A' },
  { key: '风险', emoji: '⚠', color: '#C97070' },
  { key: '重点', emoji: '📌', color: '#5E8B8B' },
  { key: '笔记', emoji: '✏', color: '#6B9080' },
] as const;

export type AnnotationAngle = 'risk' | 'outdated' | 'highlights' | 'questions';

type ListResponse = { annotations: Annotation[] };

export async function listAnnotations(itemId: string): Promise<Annotation[]> {
  try {
    const res = await api.get<ListResponse>(
      `/annotations?item_id=${encodeURIComponent(itemId)}`,
    );
    return res.annotations ?? [];
  } catch {
    return [];
  }
}

export type CreateAnnotationInput = {
  item_id: string;
  offset_start: number;
  offset_end: number;
  text_snippet: string;
  label: string;
  color?: string;
  content?: string;
};

export async function createAnnotation(
  input: CreateAnnotationInput,
): Promise<Annotation | null> {
  try {
    // 服务端 POST /annotations 返回 { id, status }（非完整 annotation），
    // 在此用入参 + 返回 id 本地组装完整 Annotation。
    const res = await api.post<{ id: string; status: string }>('/annotations', {
      ...input,
      source: 'user',
    });
    return {
      id: res.id,
      item_id: input.item_id,
      offset_start: input.offset_start,
      offset_end: input.offset_end,
      text_snippet: input.text_snippet,
      label: input.label,
      color: input.color,
      content: input.content,
      source: 'user',
      created_at: new Date().toISOString(),
    };
  } catch {
    return null;
  }
}

export async function updateAnnotation(
  id: string,
  patch: Partial<Pick<Annotation, 'content' | 'label' | 'color'>>,
): Promise<boolean> {
  try {
    await api.patch(`/annotations/${encodeURIComponent(id)}`, patch);
    return true;
  } catch {
    return false;
  }
}

export async function deleteAnnotation(id: string): Promise<boolean> {
  try {
    await api.delete(`/annotations/${encodeURIComponent(id)}`);
    return true;
  } catch {
    return false;
  }
}

/** AI 分析结果：服务端只回 created_count，故分析后重新拉全量列表回显。 */
export type AiAnalyzeResult = { created: number; annotations: Annotation[] };

export async function analyzeByAI(
  itemId: string,
  angle: AnnotationAngle,
): Promise<AiAnalyzeResult> {
  try {
    // POST /annotations/ai 返回 { status, angle, created_count, created_ids }，
    // 不含 annotation 实体 → 据 created_count 判断后重新 list 取完整数据。
    const res = await api.post<{ created_count: number }>('/annotations/ai', {
      item_id: itemId,
      angle,
    });
    const created = res.created_count ?? 0;
    const annotations = created > 0 ? await listAnnotations(itemId) : [];
    return { created, annotations };
  } catch {
    return { created: 0, annotations: [] };
  }
}
