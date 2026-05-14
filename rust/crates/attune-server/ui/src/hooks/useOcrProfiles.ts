/** useOcrProfiles — OCR Profile CRUD.
 *  对接 /api/v1/ocr/profiles.
 */
import { api } from '../store/api';
import { ocrProfiles, type OcrProfile } from '../store/signals';

export async function loadOcrProfiles(): Promise<void> {
  try {
    const list = await api.get<OcrProfile[]>('/ocr/profiles');
    ocrProfiles.value = list;
  } catch {
    ocrProfiles.value = [];
  }
}

export async function createOcrProfile(p: OcrProfile): Promise<string | null> {
  try {
    const saved = await api.post<OcrProfile>('/ocr/profiles', p);
    await loadOcrProfiles();
    return saved.id;
  } catch (e) {
    return null;
  }
}

export async function updateOcrProfile(p: OcrProfile): Promise<boolean> {
  try {
    await api.put<OcrProfile>(`/ocr/profiles/${encodeURIComponent(p.id)}`, p);
    await loadOcrProfiles();
    return true;
  } catch {
    return false;
  }
}

export async function deleteOcrProfile(id: string): Promise<boolean> {
  try {
    await api.delete(`/ocr/profiles/${encodeURIComponent(id)}`);
    await loadOcrProfiles();
    return true;
  } catch {
    return false;
  }
}
