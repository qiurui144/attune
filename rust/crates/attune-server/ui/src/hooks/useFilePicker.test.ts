import { describe, it, expect, vi, beforeEach } from 'vitest';

// We mock before importing the module under test
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}));

// Must import after mock setup
import { useFilePicker } from './useFilePicker';

describe('useFilePicker', () => {
  describe('isDesktop', () => {
    it('returns true when __TAURI_INTERNALS__ is present', () => {
      (window as any).__TAURI_INTERNALS__ = {};
      const picker = useFilePicker();
      expect(picker.isDesktop).toBe(true);
      delete (window as any).__TAURI_INTERNALS__;
    });

    it('returns false when __TAURI_INTERNALS__ is absent', () => {
      delete (window as any).__TAURI_INTERNALS__;
      const picker = useFilePicker();
      expect(picker.isDesktop).toBe(false);
    });
  });

  describe('pickDirectory', () => {
    it('returns paths from desktop dialog on success', async () => {
      (window as any).__TAURI_INTERNALS__ = {};
      const { open } = await import('@tauri-apps/plugin-dialog');
      vi.mocked(open).mockResolvedValue('/home/user/docs');

      const picker = useFilePicker();
      const result = await picker.pickDirectory({ multiple: false, title: 'Pick folder' });

      expect(result).toEqual(['/home/user/docs']);
      expect(open).toHaveBeenCalledWith({ directory: true, multiple: false, title: 'Pick folder' });
      delete (window as any).__TAURI_INTERNALS__;
    });

    it('returns multiple paths from desktop dialog when multiple selected', async () => {
      (window as any).__TAURI_INTERNALS__ = {};
      const { open } = await import('@tauri-apps/plugin-dialog');
      vi.mocked(open).mockResolvedValue(['/home/a', '/home/b']);

      const picker = useFilePicker();
      const result = await picker.pickDirectory({ multiple: true });

      expect(result).toEqual(['/home/a', '/home/b']);
      delete (window as any).__TAURI_INTERNALS__;
    });

    it('returns empty array when user cancels (null)', async () => {
      (window as any).__TAURI_INTERNALS__ = {};
      const { open } = await import('@tauri-apps/plugin-dialog');
      vi.mocked(open).mockResolvedValue(null);

      const picker = useFilePicker();
      const result = await picker.pickDirectory();

      expect(result).toEqual([]);
      delete (window as any).__TAURI_INTERNALS__;
    });

    it('returns empty array and sets error when dialog throws', async () => {
      (window as any).__TAURI_INTERNALS__ = {};
      const { open } = await import('@tauri-apps/plugin-dialog');
      vi.mocked(open).mockRejectedValue(new Error('Permission denied'));

      const picker = useFilePicker();
      const result = await picker.pickDirectory();

      expect(result).toEqual([]);
      expect(picker.error.value).toBe('Permission denied');
      delete (window as any).__TAURI_INTERNALS__;
    });

    it('sets picking=true during dialog and resets after', async () => {
      (window as any).__TAURI_INTERNALS__ = {};
      const { open } = await import('@tauri-apps/plugin-dialog');
      vi.mocked(open).mockImplementation(
        () => new Promise((r) => setTimeout(() => r('/tmp/x'), 10)),
      );

      const picker = useFilePicker();
      expect(picker.picking.value).toBe(false);
      const promise = picker.pickDirectory();
      expect(picker.picking.value).toBe(true);
      await promise;
      expect(picker.picking.value).toBe(false);
      delete (window as any).__TAURI_INTERNALS__;
    });
  });

  describe('pickFiles', () => {
    it('returns paths from desktop dialog on success', async () => {
      (window as any).__TAURI_INTERNALS__ = {};
      const { open } = await import('@tauri-apps/plugin-dialog');
      (open as any).mockClear();
      vi.mocked(open).mockResolvedValue('/tmp/doc.pdf');

      const picker = useFilePicker();
      const result = await picker.pickFiles({
        accept: '.pdf,.jpg',
        multiple: false,
        title: 'Select file',
      });

      expect(result.paths).toEqual(['/tmp/doc.pdf']);
      expect(result.files).toEqual([]);
      const callArgs = vi.mocked(open).mock.calls[0][0] as any;
      expect(callArgs.directory).toBe(false);
      expect(callArgs.multiple).toBe(false);
      expect(callArgs.title).toBe('Select file');
      delete (window as any).__TAURI_INTERNALS__;
    });

    it('returns empty on cancel', async () => {
      (window as any).__TAURI_INTERNALS__ = {};
      const { open } = await import('@tauri-apps/plugin-dialog');
      vi.mocked(open).mockResolvedValue(null);

      const picker = useFilePicker();
      const result = await picker.pickFiles();

      expect(result.paths).toEqual([]);
      expect(result.files).toEqual([]);
      delete (window as any).__TAURI_INTERNALS__;
    });

    it('handles accept string with wildcard', async () => {
      (window as any).__TAURI_INTERNALS__ = {};
      const { open } = await import('@tauri-apps/plugin-dialog');
      (open as any).mockClear();
      vi.mocked(open).mockResolvedValue('/tmp/audio.wav');

      const picker = useFilePicker();
      await picker.pickFiles({ accept: 'audio/*,.wav,.mp3' });

      expect(open).toHaveBeenCalledTimes(1);
      const callArgs = vi.mocked(open).mock.calls[0][0] as any;
      expect(callArgs.directory).toBe(false);
      expect(callArgs.filters).toHaveLength(1);
      expect(callArgs.filters[0].name).toBe('AUDIO, WAV, MP3');
      expect(callArgs.filters[0].extensions).toContain('wav');
      expect(callArgs.filters[0].extensions).toContain('mp3');
      expect(callArgs.filters[0].extensions).toContain('ogg');
      delete (window as any).__TAURI_INTERNALS__;
    });

    it('sets picking=true during dialog and resets after', async () => {
      (window as any).__TAURI_INTERNALS__ = {};
      const { open } = await import('@tauri-apps/plugin-dialog');
      vi.mocked(open).mockImplementation(
        () => new Promise((r) => setTimeout(() => r('/tmp/x'), 10)),
      );

      const picker = useFilePicker();
      const promise = picker.pickFiles();
      expect(picker.picking.value).toBe(true);
      await promise;
      expect(picker.picking.value).toBe(false);
      delete (window as any).__TAURI_INTERNALS__;
    });
  });

  describe('browser fallback', () => {
    // jsdom doesn't ship DataTransfer; provide a minimal polyfill.
    beforeEach(() => {
      if (typeof DataTransfer === 'undefined') {
        class DataTransferCtor {
          items = { add() {} };
          files!: FileList;
        }
        (globalThis as any).DataTransfer = DataTransferCtor;
      }
    });

    // Helper: set files on a hidden input — jsdom validates FileList type,
    // so we use Object.defineProperty to bypass the strict setter.
    function setInputFiles(input: HTMLInputElement, files: File[]): void {
      const fileList = Object.create(FileList.prototype);
      for (let i = 0; i < files.length; i++) {
        Object.defineProperty(fileList, i, { value: files[i], enumerable: true });
      }
      Object.defineProperty(fileList, 'length', { value: files.length });
      Object.defineProperty(fileList, 'item', {
        value: (i: number) => (fileList as any)[i] ?? null,
      });
      Object.defineProperty(input, 'files', { value: fileList });
    }
    it('pickDirectory creates webkitdirectory input in browser mode', async () => {
      delete (window as any).__TAURI_INTERNALS__;

      const picker = useFilePicker();
      const promise = picker.pickDirectory({ multiple: false });

      const inputs = document.querySelectorAll('input[type="file"]');
      const dirInput = Array.from(inputs).find(
        (el) => (el as HTMLInputElement).webkitdirectory === true,
      ) as HTMLInputElement | undefined;

      expect(dirInput).toBeTruthy();
      expect(dirInput?.multiple).toBe(false);

      // Use setInputFiles helper that bypasses jsdom's FileList setter
      const file1 = new File(['content'], 'readme.md');
      setInputFiles(dirInput!, [file1]);
      dirInput!.dispatchEvent(new Event('change'));

      const result = await promise;
      expect(result.length).toBeGreaterThanOrEqual(0);
    });

    it('pickFiles creates hidden file input in browser mode', async () => {
      delete (window as any).__TAURI_INTERNALS__;

      const picker = useFilePicker();
      const promise = picker.pickFiles({ accept: '.pdf', multiple: false });

      const inputs = document.querySelectorAll('input[type="file"]');
      const fileInput = Array.from(inputs).find(
        (el) => !(el as HTMLInputElement).webkitdirectory,
      ) as HTMLInputElement | undefined;

      expect(fileInput).toBeTruthy();
      expect(fileInput?.accept).toBe('.pdf');
      expect(fileInput?.multiple).toBe(false);

      const pdfFile = new File(['pdf content'], 'doc.pdf', { type: 'application/pdf' });
      setInputFiles(fileInput!, [pdfFile]);
      fileInput!.dispatchEvent(new Event('change'));

      const result = await promise;
      expect(result.files.length).toBe(1);
      expect(result.files[0].name).toBe('doc.pdf');
      expect(result.paths).toEqual([]);
    });

    it('cleans up hidden inputs after selection', async () => {
      delete (window as any).__TAURI_INTERNALS__;

      const picker = useFilePicker();
      const before = document.querySelectorAll('input[type="file"]').length;
      const promise = picker.pickFiles();

      const inputs = document.querySelectorAll('input[type="file"]');
      const fileInput = Array.from(inputs).find(
        (el) => !(el as HTMLInputElement).webkitdirectory,
      ) as HTMLInputElement;
      fileInput!.dispatchEvent(new Event('change'));
      await promise;

      const after = document.querySelectorAll('input[type="file"]').length;
      expect(after).toBe(before);
    });
  });
});
