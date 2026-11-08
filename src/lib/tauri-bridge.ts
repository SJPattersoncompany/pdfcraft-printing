/**
 * Tauri Bridge
 * Provides a unified interface for file operations in both Tauri and Browser environments.
 */

export interface FileFilter {
  name: string;
  extensions: string[];
}

const browserFileStore = new Map<string, File>();

export const isTauri = (): boolean => {
  return (
    typeof window !== 'undefined' &&
    ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
  );
};

/**
 * Dynamically import Tauri invoke to avoid build-time issues in non-Tauri environments
 */
async function getTauriInvoke() {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke;
}

export async function openFiles(filters: FileFilter[] = []): Promise<string[]> {
  if (isTauri()) {
    const invoke = await getTauriInvoke();
    return invoke<string[]>('open_files', { filters });
  }

  return new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = true;
    if (filters.length > 0) {
      const exts = filters.flatMap(f => f.extensions.map(ext => `.${ext}`));
      input.accept = exts.join(',');
    }

    input.onchange = () => {
      if (input.files) {
        const files = Array.from(input.files).map((file) => {
          const browserPath = `__browser__/${crypto.randomUUID()}/${file.name}`;
          browserFileStore.set(browserPath, file);
          return browserPath;
        });
        resolve(files);
      } else {
        resolve([]);
      }
    };

    input.click();
  });
}

export function getFiltersForFilename(filename: string): FileFilter[] {
  const parts = filename.split('.');
  const ext = parts.length > 1 ? parts.pop()?.toLowerCase() : undefined;
  if (!ext) {
    return [{ name: 'All Files', extensions: ['*'] }];
  }

  const extMap: Record<string, string> = {
    pdf: 'PDF Document',
    xlsx: 'Excel Spreadsheet',
    xls: 'Excel Spreadsheet',
    docx: 'Word Document',
    doc: 'Word Document',
    pptx: 'PowerPoint Presentation',
    ppt: 'PowerPoint Presentation',
    png: 'PNG Image',
    jpg: 'JPEG Image',
    jpeg: 'JPEG Image',
    webp: 'WebP Image',
    svg: 'SVG Image',
    zip: 'ZIP Archive',
    txt: 'Text Document',
    json: 'JSON Document',
    csv: 'CSV Document',
  };

  const label = extMap[ext] || `${ext.toUpperCase()} File`;
  return [
    { name: label, extensions: [ext] },
    { name: 'All Files', extensions: ['*'] },
  ];
}

export async function saveFile(suggestedName: string, filters: FileFilter[] = []): Promise<string | null> {
  if (isTauri()) {
    try {
      const invoke = await getTauriInvoke();
      return await invoke<string>('save_file', { suggestedName, filters });
    } catch (err: unknown) {
      const errMsg = err instanceof Error ? err.message : String(err);
      if (
        errMsg.includes('No file selected') ||
        errMsg.includes('canceled') ||
        errMsg.includes('cancelled')
      ) {
        return null;
      }
      // Fallback: attempt to use @tauri-apps/plugin-dialog save
      try {
        const { save } = await import('@tauri-apps/plugin-dialog');
        const res = await save({
          defaultPath: suggestedName,
          filters: filters.map((f) => ({ name: f.name, extensions: f.extensions })),
        });
        return res;
      } catch {
        throw err;
      }
    }
  }

  // Browser fallback: no native save path API, use filename as virtual path.
  return suggestedName.replace(/^[/\\]+/, '');
}

export async function readFileBytes(path: string): Promise<Uint8Array> {
  if (isTauri()) {
    const invoke = await getTauriInvoke();
    const data = await invoke<number[]>('read_file', { path });
    return new Uint8Array(data);
  }

  const file = browserFileStore.get(path);
  if (!file) {
    throw new Error(`File not found in browser session: ${path}`);
  }

  const buffer = await file.arrayBuffer();
  return new Uint8Array(buffer);
}

export async function writeFileBytes(path: string, data: Uint8Array): Promise<void> {
  if (isTauri()) {
    try {
      const invoke = await getTauriInvoke();
      await invoke('write_file', { path, data: Array.from(data) });
      return;
    } catch (err) {
      try {
        const { writeFile } = await import('@tauri-apps/plugin-fs');
        await writeFile(path, data);
        return;
      } catch {
        throw err;
      }
    }
  }

  const blobData = new Uint8Array(data.byteLength);
  blobData.set(data);
  const blob = new Blob([blobData.buffer]);
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  const filename = path.split('/').pop() || path.split('\\').pop() || 'download.bin';
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

/**
 * Saves a Blob or byte array to disk.
 * - In Tauri desktop environment: opens native OS Save File dialog for the user to choose
 *   the destination directory and filename, then writes the file bytes.
 * - In Browser environment: triggers standard browser download via temporary <a> element.
 *
 * @param fileData Blob or Uint8Array containing file content
 * @param suggestedName Suggested filename (e.g., 'converted.pdf')
 * @param filters Optional file dialog filters
 * @returns Promise<boolean> Returns true if file was saved / downloaded, false if cancelled by user.
 */
export async function saveBlobFile(
  fileData: Blob | Uint8Array,
  suggestedName: string,
  filters?: FileFilter[]
): Promise<boolean> {
  const activeFilters =
    filters && filters.length > 0 ? filters : getFiltersForFilename(suggestedName);

  if (isTauri()) {
    const selectedPath = await saveFile(suggestedName, activeFilters);
    if (!selectedPath) {
      // User cancelled dialog
      return false;
    }

    let bytes: Uint8Array;
    if (fileData instanceof Uint8Array) {
      bytes = fileData;
    } else {
      const buffer = await fileData.arrayBuffer();
      bytes = new Uint8Array(buffer);
    }

    await writeFileBytes(selectedPath, bytes);
    return true;
  }

  // Browser fallback: <a> tag download
  let blob: Blob;
  if (fileData instanceof Blob) {
    blob = fileData;
  } else {
    const copy = new Uint8Array(fileData.byteLength);
    copy.set(fileData);
    blob = new Blob([copy.buffer]);
  }

  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = suggestedName;
  a.style.display = 'none';
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  setTimeout(() => {
    URL.revokeObjectURL(url);
  }, 1000);
  return true;
}

/**
 * Opens a URL in the system's default web browser.
 * In a desktop Tauri environment, it invokes the native open_url command.
 * In a standard browser environment, it falls back to window.open or anchor click.
 */
export async function openExternalUrl(url: string): Promise<boolean> {
  if (!url) return false;

  if (isTauri()) {
    try {
      const invoke = await getTauriInvoke();
      await invoke('open_url', { url });
      return true;
    } catch (err) {
      console.warn('Tauri native open_url failed, trying browser fallbacks:', err);
    }
  }

  if (typeof window !== 'undefined') {
    try {
      const win = window.open(url, '_blank', 'noopener,noreferrer');
      if (win) return true;
    } catch {
      // ignore
    }

    try {
      const a = document.createElement('a');
      a.href = url;
      a.target = '_blank';
      a.rel = 'noopener noreferrer';
      document.body.appendChild(a);
      a.click();
      setTimeout(() => {
        document.body.removeChild(a);
      }, 100);
      return true;
    } catch (e) {
      console.error('Failed to open link via fallback anchor:', e);
    }
  }

  return false;
}

export interface SystemPrinter {
  name: string;
  is_default: boolean;
  port_name?: string | null;
}

/**
 * Fetch installed system printers (default printer first).
 * Desktop app only; throws if the operating system could not be queried.
 * In a standard browser environment: returns an empty array.
 */
export async function listSystemPrinters(): Promise<SystemPrinter[]> {
  if (!isTauri()) return [];
  const invoke = await getTauriInvoke();
  return await invoke<SystemPrinter[]>('get_system_printers');
}

/**
 * Like {@link listSystemPrinters} but never throws (returns an empty list on failure).
 */
export async function getSystemPrinters(): Promise<SystemPrinter[]> {
  try {
    return await listSystemPrinters();
  } catch (err) {
    console.warn('Failed to get system printers via Tauri:', err);
    return [];
  }
}

export interface PrintOptions {
  /** Number of collated copies (1-99). */
  copies?: number;
  /** Print on both sides of the paper (long-edge binding). */
  duplex?: boolean;
  /** Document name shown in the printer queue. */
  jobName?: string;
}

/**
 * Print a PDF document natively or via browser fallback.
 * In desktop Tauri environment: sends the PDF straight to the given printer (or the
 * system default printer). The bytes go as a raw IPC body, options as headers.
 * In browser environment: falls back to window.print().
 */
export async function printPdfFile(
  fileData: Blob | Uint8Array,
  printerName?: string,
  options: PrintOptions = {}
): Promise<{ success: boolean; error?: string }> {
  if (isTauri()) {
    try {
      const invoke = await getTauriInvoke();
      const bytes =
        fileData instanceof Uint8Array ? fileData : new Uint8Array(await fileData.arrayBuffer());
      const copies = Math.min(99, Math.max(1, Math.floor(options.copies ?? 1)));
      const headers: Record<string, string> = {
        'x-copies': String(copies),
        'x-duplex': options.duplex ? '1' : '0',
      };
      if (printerName) headers['x-printer-name'] = encodeURIComponent(printerName);
      if (options.jobName) headers['x-job-name'] = encodeURIComponent(options.jobName);
      await invoke('print_pdf_file', bytes, { headers });
      return { success: true };
    } catch (err: unknown) {
      const errMsg = err instanceof Error ? err.message : String(err);
      return { success: false, error: errMsg };
    }
  }

  // Browser fallback
  if (typeof window !== 'undefined') {
    window.print();
    return { success: true };
  }

  return { success: false, error: 'Environment does not support printing' };
}

/**
 * Open the operating system's printer setup (desktop app only).
 * `add` opens the Add Printer wizard where available (Windows), `manage` the printer
 * settings page.
 */
export async function openPrinterSettings(
  mode: 'add' | 'manage' = 'add'
): Promise<{ success: boolean; error?: string }> {
  if (!isTauri()) {
    return { success: false, error: 'Printer setup is only available in the desktop app' };
  }
  try {
    const invoke = await getTauriInvoke();
    await invoke('open_printer_settings', { mode });
    return { success: true };
  } catch (err: unknown) {
    return { success: false, error: err instanceof Error ? err.message : String(err) };
  }
}


// ---------------------------------------------------------------------------
// PDFs opened from the operating system (double-click, "Open with", second launch)
// ---------------------------------------------------------------------------

/** Emitted by the desktop app whenever new files are waiting to be opened. */
export const FILES_OPENED_EVENT = 'pdfcraft://files-opened';

/** True when the desktop app has PDFs waiting to be opened (does not take them). */
export async function hasOpenedFiles(): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    const invoke = await getTauriInvoke();
    return await invoke<boolean>('has_opened_files');
  } catch {
    return false;
  }
}

function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || 'document.pdf';
}

/**
 * Takes the PDFs waiting to be opened and reads them into File objects.
 * Files that can no longer be read are reported in `errors`.
 */
function toBlobPart(data: unknown): BlobPart | null {
  if (data instanceof ArrayBuffer) return data;
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (Array.isArray(data) && data.every((n) => typeof n === 'number')) {
    return new Uint8Array(data);
  }
  return null;
}

export async function takeOpenedFiles(): Promise<{ files: File[]; errors: string[] }> {
  if (!isTauri()) return { files: [], errors: [] };
  const invoke = await getTauriInvoke();
  const batch = await invoke<{ paths?: string[]; errors?: string[] }>('take_opened_files');
  const files: File[] = [];
  const errors: string[] = [...(batch.errors ?? [])];
  for (const path of batch.paths ?? []) {
    try {
      const data = await invoke<unknown>('read_file_binary', { path });
      const bytes = toBlobPart(data);
      if (!bytes) {
        errors.push(`Could not read "${baseName(path)}".`);
        continue;
      }
      files.push(new File([bytes], baseName(path), { type: 'application/pdf' }));
    } catch (err) {
      errors.push(err instanceof Error ? err.message : String(err));
    }
  }
  return { files, errors };
}

/** Calls `handler` whenever the desktop app receives new files. Returns an unsubscribe function. */
export async function onFilesOpened(handler: () => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  try {
    const { listen } = await import('@tauri-apps/api/event');
    return await listen(FILES_OPENED_EVENT, () => handler());
  } catch {
    return () => {};
  }
}
