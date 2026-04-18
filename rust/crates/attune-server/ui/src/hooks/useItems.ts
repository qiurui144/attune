/** useItems · 条目列表加载 */
import { api } from '../store/api';
import { items } from '../store/signals';
import type { Item } from '../store/signals';

type ListResponse = { items: Item[]; count: number };

export type DecryptedItem = {
  id: string;
  title: string;
  content: string;
  url?: string;
  source_type: string;
  domain?: string;
  tags?: string[];
  created_at: string;
  updated_at: string;
};

export async function loadItems(limit = 50, offset = 0): Promise<void> {
  try {
    const res = await api.get<ListResponse>(`/items?limit=${limit}&offset=${offset}`);
    items.value = res.items ?? [];
  } catch {
    items.value = [];
  }
}

export async function getItem(id: string): Promise<DecryptedItem | null> {
  try {
    return await api.get<DecryptedItem>(`/items/${encodeURIComponent(id)}`);
  } catch {
    return null;
  }
}

export async function deleteItem(id: string): Promise<boolean> {
  try {
    await api.delete(`/items/${encodeURIComponent(id)}`);
    items.value = items.value.filter((it) => it.id !== id);
    return true;
  } catch {
    return false;
  }
}
