/** useAnnotations · 批注 CRUD + AI 4 角度 */
import { api } from '../store/api';

export type Annotation = {
  id: string;
  item_id: string;
  start_offset: number;
  end_offset: number;
  snippet: string;
  tag: string; // 疑问/灵感/风险/重点/笔记
  color?: string;
  note?: string;
  source: 'user' | 'ai';
  angle?: string; // AI 批注的角度
  model?: string;
  confidence?: number;
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
  start_offset: number;
  end_offset: number;
  snippet: string;
  tag: string;
  color?: string;
  note?: string;
};

export async function createAnnotation(
  input: CreateAnnotationInput,
): Promise<Annotation | null> {
  try {
    const res = await api.post<{ annotation: Annotation }>('/annotations', {
      ...input,
      source: 'user',
    });
    return res.annotation;
  } catch {
    return null;
  }
}

export async function updateAnnotation(
  id: string,
  patch: Partial<Pick<Annotation, 'note' | 'tag' | 'color'>>,
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

export async function analyzeByAI(
  itemId: string,
  angle: AnnotationAngle,
): Promise<Annotation[]> {
  try {
    const res = await api.post<{ annotations: Annotation[] }>('/annotations/ai', {
      item_id: itemId,
      angle,
    });
    return res.annotations ?? [];
  } catch {
    return [];
  }
}
