'use client';

import { useEffect, useRef } from 'react';
import { usePathname, useRouter } from 'next/navigation';
import { hasOpenedFiles, isTauri, onFilesOpened } from '@/lib/tauri-bridge';

/**
 * Desktop app only. When the operating system hands PDFCraft a PDF (double-click,
 * "Open with", or a second launch while the app is running) and the user is not in
 * Studio, switch to Studio, which opens each file as a new tab.
 */
export function DesktopFileOpenHandler({ locale }: { locale: string }) {
  const router = useRouter();
  const pathname = usePathname();
  const pathnameRef = useRef(pathname);
  pathnameRef.current = pathname;

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;

    const goToStudio = async () => {
      if (pathnameRef.current?.includes('/studio')) return; // Studio picks the files up itself
      if (await hasOpenedFiles()) {
        router.push(`/${locale}/studio/`);
      }
    };

    goToStudio();
    onFilesOpened(goToStudio).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [locale, router]);

  return null;
}

export default DesktopFileOpenHandler;
