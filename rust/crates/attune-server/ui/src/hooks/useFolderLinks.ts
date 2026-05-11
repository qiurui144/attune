/** useFolderLinks — 只读关联本地目录列表.
 *  对接 GET /api/v1/folder-links (写入由 attune-cli link-folder).
 */
import { api } from '../store/api';
import { folderLinks, type FolderLink } from '../store/signals';

type Response = { folder_links?: FolderLink[] } | FolderLink[];

export async function loadFolderLinks(): Promise<void> {
  try {
    const raw = await api.get<Response>('/folder-links');
    folderLinks.value = Array.isArray(raw) ? raw : raw.folder_links ?? [];
  } catch {
    folderLinks.value = [];
  }
}
