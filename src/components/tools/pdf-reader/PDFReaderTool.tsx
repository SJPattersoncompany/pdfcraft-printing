'use client';

import React, { useState, useCallback, useRef, useEffect } from 'react';
import { useTranslations, useLocale } from 'next-intl';
import { FileUploader } from '../FileUploader';
import { Button } from '@/components/ui/Button';
import { Card } from '@/components/ui/Card';
import { Download, X, CopyCheck, Printer } from 'lucide-react';
import { saveBlobFile, isTauri } from '@/lib/tauri-bridge';
import { PrintDialog, SELECTED_PRINTER_STORAGE_KEY } from '@/components/print/PrintDialog';
import { withBasePath } from '@/lib/utils/path';

export { SELECTED_PRINTER_STORAGE_KEY };

export interface PDFReaderToolProps {
    className?: string;
}

/**
 * PDFReaderTool Component
 * 
 * A full-featured PDF reader with automatic selection-to-clipboard synchronization.
 */
export function PDFReaderTool({ className = '' }: PDFReaderToolProps) {
    const t = useTranslations('common');
    const tTools = useTranslations('tools');

    const [file, setFile] = useState<File | null>(null);
    const [pdfUrl, setPdfUrl] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [autoCopyEnabled, setAutoCopyEnabled] = useState(true);
    const [copiedNotify, setCopiedNotify] = useState<string | null>(null);

    // Native printing state
    const [isPrintModalOpen, setIsPrintModalOpen] = useState(false);
    const locale = useLocale();
    const isZh = locale?.toLowerCase().startsWith('zh') ?? false;

    const containerRef = useRef<HTMLDivElement>(null);
    const iframeRef = useRef<HTMLIFrameElement>(null);
    const lastCopiedRef = useRef<string>('');

    // Sync selected text to clipboard
    const syncSelection = useCallback((targetWindow?: Window | null) => {
        if (!autoCopyEnabled) return;
        try {
            const win = targetWindow || window;
            const sel = win.getSelection();
            const text = sel ? sel.toString().trim() : '';
            if (text && text.length > 0 && text !== lastCopiedRef.current) {
                lastCopiedRef.current = text;
                if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
                    navigator.clipboard.writeText(text).then(() => {
                        setCopiedNotify(text.slice(0, 30) + (text.length > 30 ? '...' : ''));
                        setTimeout(() => setCopiedNotify(null), 2000);
                    }).catch(() => {});
                }
            }
        } catch {}
    }, [autoCopyEnabled]);

    // Attach selection listener to host document
    useEffect(() => {
        const handleMouseUp = () => syncSelection(window);
        document.addEventListener('mouseup', handleMouseUp);
        return () => document.removeEventListener('mouseup', handleMouseUp);
    }, [syncSelection]);

    // Attach selection listener into iframe contentDocument once loaded
    const handleIframeLoad = useCallback(() => {
        try {
            const iframe = iframeRef.current;
            if (iframe?.contentDocument && iframe.contentWindow) {
                const doc = iframe.contentDocument;
                const win = iframe.contentWindow;
                doc.addEventListener('mouseup', () => syncSelection(win));
                doc.addEventListener('touchend', () => syncSelection(win));
            }
        } catch {}
    }, [syncSelection]);

    const handleFilesSelected = useCallback((files: File[]) => {
        if (files.length > 0) {
            const selectedFile = files[0];
            setFile(selectedFile);

            // Revoke previous URL
            if (pdfUrl) {
                URL.revokeObjectURL(pdfUrl);
            }

            // Create new URL for the PDF
            const url = URL.createObjectURL(selectedFile);
            setPdfUrl(url);
            setError(null);
        }
    }, [pdfUrl]);

    const handleUploadError = useCallback((errorMessage: string) => {
        setError(errorMessage);
    }, []);

    // Cleanup URL on unmount
    useEffect(() => {
        return () => {
            if (pdfUrl) {
                URL.revokeObjectURL(pdfUrl);
            }
        };
    }, [pdfUrl]);

    const handleDownload = useCallback(() => {
        if (file) {
            saveBlobFile(file, file.name);
        }
    }, [file]);

    const handleTriggerPrint = useCallback(() => {
        if (!file) return;

        if (!isTauri()) {
            // Web browser fallback
            try {
                if (iframeRef.current?.contentWindow) {
                    iframeRef.current.contentWindow.print();
                } else {
                    window.print();
                }
            } catch {
                window.print();
            }
            return;
        }

        // Native Tauri desktop: open the printer selection dialog
        setIsPrintModalOpen(true);
    }, [file]);

    // Listen for print requests emitted from PDF.js viewer iframe
    useEffect(() => {
        const handleMessage = (event: MessageEvent) => {
            if (event.data?.type === 'pdfcraft-print-requested') {
                handleTriggerPrint();
            }
        };
        window.addEventListener('message', handleMessage);
        return () => window.removeEventListener('message', handleMessage);
    }, [handleTriggerPrint]);

    // Listen for Ctrl+P / Cmd+P print shortcut
    useEffect(() => {
        const handleKeyDown = (event: KeyboardEvent) => {
            if (!file) return;

            if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === 'p') {
                event.preventDefault();
                if (!isPrintModalOpen) {
                    handleTriggerPrint();
                }
            }
        };

        window.addEventListener('keydown', handleKeyDown);
        return () => window.removeEventListener('keydown', handleKeyDown);
    }, [file, isPrintModalOpen, handleTriggerPrint]);

    const handleReset = useCallback(() => {
        if (pdfUrl) {
            URL.revokeObjectURL(pdfUrl);
        }
        setFile(null);
        setPdfUrl(null);
        setError(null);
        setCopiedNotify(null);
        setIsPrintModalOpen(false);
    }, [pdfUrl]);

    const hasFile = file !== null;

    return (
        <div className={`space-y-4 ${className}`.trim()} ref={containerRef}>
            {!hasFile && (
                <FileUploader
                    accept={['application/pdf', '.pdf']}
                    multiple={false}
                    maxFiles={1}
                    onFilesSelected={handleFilesSelected}
                    onError={handleUploadError}
                    label={tTools('pdfReader.uploadLabel') || 'Open PDF File'}
                    description={tTools('pdfReader.uploadDescription') || 'Open a PDF file to read and view.'}
                />
            )}

            {error && (
                <div className="p-4 rounded-[var(--radius-md)] bg-red-50 border border-red-200 text-red-700" role="alert">
                    <p className="text-sm">{error}</p>
                </div>
            )}

            {hasFile && pdfUrl && (
                <>
                    {/* Simple Header with File Info */}
                    <Card variant="outlined" className="!p-3">
                        <div className="flex items-center justify-between">
                            <div className="flex items-center gap-3">
                                <span className="text-sm font-medium text-[hsl(var(--color-foreground))]">
                                    {file.name}
                                </span>
                                <span className="text-xs text-[hsl(var(--color-muted-foreground))]">
                                    ({(file.size / 1024 / 1024).toFixed(2)} MB)
                                </span>
                            </div>
                            <div className="flex items-center gap-2">
                                {/* Auto-copy selection toggle (Ubuntu / Linux Primary Selection feature) */}
                                <Button
                                    variant={autoCopyEnabled ? "secondary" : "ghost"}
                                    size="sm"
                                    onClick={() => setAutoCopyEnabled(!autoCopyEnabled)}
                                    className="text-xs h-7 gap-1"
                                    title={isZh
                                        ? (autoCopyEnabled ? "划选自动存入剪贴板 (已开启)" : "划选自动存入剪贴板 (已关闭)")
                                        : (autoCopyEnabled ? "Copy selected text automatically (on)" : "Copy selected text automatically (off)")}
                                >
                                    <CopyCheck className={`w-3.5 h-3.5 ${autoCopyEnabled ? 'text-primary' : 'text-muted-foreground'}`} />
                                    <span className="hidden sm:inline">{isZh ? "选中文本自动复制" : "Auto-copy selection"}</span>
                                </Button>

                                <Button
                                    variant="ghost"
                                    size="sm"
                                    onClick={handleTriggerPrint}
                                    title={isZh ? "打印 (Ctrl+P)" : "Print (Ctrl+P)"}
                                >
                                    <Printer className="w-4 h-4" />
                                </Button>
                                <Button variant="ghost" size="sm" onClick={handleDownload} title="Download">
                                    <Download className="w-4 h-4" />
                                </Button>
                                <Button variant="ghost" size="sm" onClick={handleReset} title="Close">
                                    <X className="w-4 h-4" />
                                </Button>
                            </div>
                        </div>
                    </Card>

                    {/* Copied to clipboard toast feedback */}
                    {copiedNotify && (
                        <div className="fixed bottom-6 right-6 z-50 px-3 py-2 rounded-lg bg-foreground text-background text-xs shadow-lg flex items-center gap-2 animate-in fade-in slide-in-from-bottom-2">
                            <CopyCheck className="w-3.5 h-3.5 text-emerald-400" />
                            <span>{isZh ? "已自动存入剪贴板: " : "Copied to clipboard: "}<strong>{copiedNotify}</strong></span>
                        </div>
                    )}

                    {/* PDF Viewer - Using full-featured PDF.js Viewer */}
                    <div
                        className="relative bg-gray-100 rounded-[var(--radius-md)] overflow-hidden"
                        style={{ height: '80vh', minHeight: '600px' }}
                    >
                        <iframe
                            ref={iframeRef}
                            src={withBasePath(`/pdfjs-viewer/viewer.html?file=${encodeURIComponent(pdfUrl)}`)}
                            onLoad={handleIframeLoad}
                            className="w-full h-full absolute inset-0 border-0"
                            title={file.name}
                        />
                    </div>

                    {/* Native printer selection dialog (desktop app) */}
                    <PrintDialog
                        isOpen={isPrintModalOpen}
                        onClose={() => setIsPrintModalOpen(false)}
                        file={file}
                        documentName={file.name}
                        locale={locale}
                    />
                </>
            )}
        </div>
    );
}

export default PDFReaderTool;
