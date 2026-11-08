'use client';

import React, { useCallback, useEffect, useRef, useState } from 'react';
import {
  AlertCircle,
  CheckCircle2,
  Loader2,
  Plus,
  Printer,
  RefreshCw,
  Settings,
} from 'lucide-react';
import { Button } from '@/components/ui/Button';
import { Modal } from '@/components/ui';
import {
  listSystemPrinters,
  openPrinterSettings,
  printPdfFile,
  type SystemPrinter,
} from '@/lib/tauri-bridge';

export const SELECTED_PRINTER_STORAGE_KEY = 'pdfcraft-selected-printer';
export const PRINT_DUPLEX_STORAGE_KEY = 'pdfcraft-print-duplex';

const STRINGS = {
  en: {
    title: 'Print',
    searching: 'Looking for printers…',
    noPrinters: 'No printers found',
    noPrintersHint:
      'Add your printer to this computer, then click Refresh. If the printer is already set up, make sure it is turned on and connected.',
    listError: 'Could not read the printer list',
    printer: 'Printer',
    default: 'default',
    copies: 'Copies',
    duplex: 'Print on both sides',
    addPrinter: 'Add a printer…',
    printerSettings: 'Printer settings',
    addHint: 'Finish adding the printer in the window that opened, then come back here. The list refreshes automatically.',
    refresh: 'Refresh',
    cancel: 'Cancel',
    print: 'Print',
    printing: 'Printing…',
    sentTo: (p: string) => `Sent to "${p}"`,
    sentDefault: 'Sent to the default printer',
    failed: 'Printing failed. Check that the printer is on and try again.',
  },
  zh: {
    title: '打印',
    searching: '正在检索打印机…',
    noPrinters: '未检测到打印机',
    noPrintersHint: '请先在系统中添加打印机，然后点击刷新。如已添加，请确认打印机已开机并连接。',
    listError: '无法读取打印机列表',
    printer: '打印机',
    default: '默认',
    copies: '份数',
    duplex: '双面打印',
    addPrinter: '添加打印机…',
    printerSettings: '打印机设置',
    addHint: '请在打开的窗口中完成添加，然后回到这里，列表会自动刷新。',
    refresh: '刷新',
    cancel: '取消',
    print: '打印',
    printing: '正在打印…',
    sentTo: (p: string) => `已发送到 "${p}"`,
    sentDefault: '已发送到系统默认打印机',
    failed: '打印失败，请检查打印机状态后重试。',
  },
};

export interface PrintDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** The PDF to print. */
  file: Blob | null;
  /** Name shown in the printer queue. */
  documentName?: string;
  /** UI language (e.g. "en", "zh"); defaults to the page language. */
  locale?: string;
}

function readStorage(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* storage unavailable */
  }
}

/**
 * Native print dialog for the desktop app: pick a printer, copies and two-sided printing,
 * or open the operating system's Add Printer wizard.
 */
export function PrintDialog({ isOpen, onClose, file, documentName, locale }: PrintDialogProps) {
  const lang =
    locale ?? (typeof document !== 'undefined' ? document.documentElement.lang : '') ?? '';
  const s = lang.toLowerCase().startsWith('zh') ? STRINGS.zh : STRINGS.en;

  const [printers, setPrinters] = useState<SystemPrinter[]>([]);
  const [selectedPrinter, setSelectedPrinter] = useState('');
  const [copies, setCopies] = useState(1);
  const [duplex, setDuplex] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [isPrinting, setIsPrinting] = useState(false);
  const [listError, setListError] = useState<string | null>(null);
  const [status, setStatus] = useState<{ type: 'success' | 'error' | 'info'; message: string } | null>(null);
  const waitingForSetup = useRef(false);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const loadPrinters = useCallback(async () => {
    setIsLoading(true);
    setListError(null);
    try {
      const list = await listSystemPrinters();
      setPrinters(list);
      setSelectedPrinter((current) => {
        if (current && list.some((p) => p.name === current)) return current;
        const saved = readStorage(SELECTED_PRINTER_STORAGE_KEY);
        if (saved && list.some((p) => p.name === saved)) return saved;
        return list.find((p) => p.is_default)?.name ?? list[0]?.name ?? '';
      });
    } catch (err) {
      setPrinters([]);
      setListError(err instanceof Error ? err.message : String(err));
    } finally {
      setIsLoading(false);
    }
  }, []);

  // Load when opened.
  useEffect(() => {
    if (!isOpen) return;
    setStatus(null);
    setCopies(1);
    setDuplex(readStorage(PRINT_DUPLEX_STORAGE_KEY) === '1');
    loadPrinters();
  }, [isOpen, loadPrinters]);

  // After the user went off to add a printer, refresh when they come back to the app.
  useEffect(() => {
    if (!isOpen) return;
    const onFocus = () => {
      if (waitingForSetup.current) {
        waitingForSetup.current = false;
        loadPrinters();
      }
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [isOpen, loadPrinters]);

  useEffect(() => () => {
    if (closeTimer.current) clearTimeout(closeTimer.current);
  }, []);

  const handleOpenSettings = useCallback(
    async (mode: 'add' | 'manage') => {
      const res = await openPrinterSettings(mode);
      if (res.success) {
        waitingForSetup.current = true;
        setStatus({ type: 'info', message: s.addHint });
      } else {
        setStatus({ type: 'error', message: res.error || s.failed });
      }
    },
    [s]
  );

  const handlePrinterChange = useCallback((name: string) => {
    setSelectedPrinter(name);
    writeStorage(SELECTED_PRINTER_STORAGE_KEY, name);
  }, []);

  const handlePrint = useCallback(async () => {
    if (!file || !selectedPrinter) return;
    writeStorage(SELECTED_PRINTER_STORAGE_KEY, selectedPrinter);
    writeStorage(PRINT_DUPLEX_STORAGE_KEY, duplex ? '1' : '0');
    setIsPrinting(true);
    setStatus(null);
    const res = await printPdfFile(file, selectedPrinter, {
      copies,
      duplex,
      jobName: documentName || 'PDFCraft document',
    });
    setIsPrinting(false);
    if (res.success) {
      setStatus({ type: 'success', message: s.sentTo(selectedPrinter) });
      closeTimer.current = setTimeout(() => {
        setStatus(null);
        onClose();
      }, 1500);
    } else {
      setStatus({ type: 'error', message: res.error || s.failed });
    }
  }, [file, selectedPrinter, copies, duplex, documentName, onClose, s]);

  const statusBox = status && (
    <div
      role={status.type === 'error' ? 'alert' : 'status'}
      className={`p-3 rounded-lg text-sm flex items-start gap-2 border ${
        status.type === 'success'
          ? 'bg-emerald-50 dark:bg-emerald-950/30 text-emerald-700 dark:text-emerald-300 border-emerald-200 dark:border-emerald-800'
          : status.type === 'error'
            ? 'bg-red-50 dark:bg-red-950/30 text-red-700 dark:text-red-300 border-red-200 dark:border-red-800'
            : 'bg-sky-50 dark:bg-sky-950/30 text-sky-800 dark:text-sky-200 border-sky-200 dark:border-sky-800'
      }`}
    >
      {status.type === 'success' ? (
        <CheckCircle2 className="w-4 h-4 flex-shrink-0 mt-0.5" />
      ) : (
        <AlertCircle className="w-4 h-4 flex-shrink-0 mt-0.5" />
      )}
      <span className="break-words">{status.message}</span>
    </div>
  );

  const setupButtons = (
    <div className="flex flex-wrap items-center gap-2">
      <Button variant="outline" size="sm" onClick={() => handleOpenSettings('add')} disabled={isPrinting}>
        <Plus className="w-3.5 h-3.5 mr-1" />
        {s.addPrinter}
      </Button>
      <Button variant="ghost" size="sm" onClick={() => handleOpenSettings('manage')} disabled={isPrinting}>
        <Settings className="w-3.5 h-3.5 mr-1" />
        {s.printerSettings}
      </Button>
    </div>
  );

  return (
    <Modal isOpen={isOpen} onClose={() => !isPrinting && onClose()} title={s.title} size="md">
      <div className="space-y-4 py-2">
        {isLoading ? (
          <div className="flex flex-col items-center justify-center py-8 gap-3 text-muted-foreground">
            <Loader2 className="w-6 h-6 animate-spin text-primary" />
            <span className="text-sm">{s.searching}</span>
          </div>
        ) : printers.length === 0 ? (
          <div className="space-y-3">
            <div className="p-4 rounded-lg bg-amber-50 dark:bg-amber-950/20 border border-amber-200 dark:border-amber-900/30 text-amber-800 dark:text-amber-200 text-sm flex items-start gap-3">
              <AlertCircle className="w-5 h-5 flex-shrink-0 mt-0.5" />
              <div>
                <p className="font-medium">{listError ? s.listError : s.noPrinters}</p>
                <p className="text-xs mt-1 text-amber-700 dark:text-amber-300">{listError || s.noPrintersHint}</p>
              </div>
            </div>
            {statusBox}
            <div className="flex flex-wrap items-center justify-between gap-2">
              {setupButtons}
              <Button variant="outline" size="sm" onClick={loadPrinters}>
                <RefreshCw className="w-3.5 h-3.5 mr-1" />
                {s.refresh}
              </Button>
            </div>
          </div>
        ) : (
          <div className="space-y-4">
            <div>
              <label
                htmlFor="pdfcraft-printer-select"
                className="block text-xs font-semibold text-muted-foreground uppercase tracking-wider mb-2"
              >
                {s.printer}
              </label>
              <div className="flex gap-2">
                <select
                  id="pdfcraft-printer-select"
                  value={selectedPrinter}
                  onChange={(e) => handlePrinterChange(e.target.value)}
                  disabled={isPrinting}
                  className="flex-1 min-w-0 px-3 py-2 text-sm rounded-md border border-[hsl(var(--color-input))] bg-[hsl(var(--color-background))] text-[hsl(var(--color-foreground))] focus:outline-none focus:ring-2 focus:ring-primary focus:border-transparent"
                >
                  {printers.map((p) => (
                    <option key={p.name} value={p.name}>
                      {p.is_default ? `${p.name} (${s.default})` : p.name}
                    </option>
                  ))}
                </select>
                <Button variant="ghost" size="sm" onClick={loadPrinters} disabled={isPrinting} title={s.refresh} aria-label={s.refresh}>
                  <RefreshCw className="w-4 h-4" />
                </Button>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-6">
              <label className="flex items-center gap-2 text-sm">
                <span className="text-muted-foreground">{s.copies}</span>
                <input
                  type="number"
                  min={1}
                  max={99}
                  value={copies}
                  disabled={isPrinting}
                  onChange={(e) => setCopies(Math.min(99, Math.max(1, Number(e.target.value) || 1)))}
                  className="w-16 px-2 py-1 text-sm rounded-md border border-[hsl(var(--color-input))] bg-[hsl(var(--color-background))] text-[hsl(var(--color-foreground))]"
                />
              </label>
              <label className="flex items-center gap-2 text-sm cursor-pointer">
                <input
                  type="checkbox"
                  checked={duplex}
                  disabled={isPrinting}
                  onChange={(e) => setDuplex(e.target.checked)}
                />
                <span>{s.duplex}</span>
              </label>
            </div>

            {statusBox}

            <div className="flex flex-wrap items-center justify-between gap-2 pt-3 border-t border-[hsl(var(--color-border))]">
              {setupButtons}
              <div className="flex items-center gap-2">
                <Button variant="outline" size="sm" onClick={onClose} disabled={isPrinting}>
                  {s.cancel}
                </Button>
                <Button variant="primary" size="sm" onClick={handlePrint} disabled={isPrinting || !selectedPrinter || !file}>
                  {isPrinting ? (
                    <>
                      <Loader2 className="w-4 h-4 animate-spin mr-1" />
                      {s.printing}
                    </>
                  ) : (
                    <>
                      <Printer className="w-4 h-4 mr-1" />
                      {s.print}
                    </>
                  )}
                </Button>
              </div>
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
}

export default PrintDialog;
