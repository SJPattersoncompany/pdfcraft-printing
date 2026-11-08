import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, waitFor, cleanup, act } from '@testing-library/react';
import React from 'react';

const push = vi.fn();
let pathname = '/en/tools/pdf-reader/';
vi.mock('next/navigation', () => ({
  useRouter: () => ({ push }),
  usePathname: () => pathname,
}));

let handler: (() => void) | undefined;
vi.mock('@/lib/tauri-bridge', () => ({
  isTauri: vi.fn(() => true),
  hasOpenedFiles: vi.fn(),
  onFilesOpened: vi.fn(async (h: () => void) => {
    handler = h;
    return () => {};
  }),
}));

import { DesktopFileOpenHandler } from '@/components/desktop/DesktopFileOpenHandler';
import * as bridge from '@/lib/tauri-bridge';

describe('DesktopFileOpenHandler', () => {
  beforeEach(() => {
    push.mockClear();
    handler = undefined;
    pathname = '/en/tools/pdf-reader/';
  });
  afterEach(() => cleanup());

  it('switches to Studio when the app was launched with a PDF', async () => {
    vi.mocked(bridge.hasOpenedFiles).mockResolvedValue(true);
    render(<DesktopFileOpenHandler locale="en" />);
    await waitFor(() => expect(push).toHaveBeenCalledWith('/en/studio/'));
  });

  it('switches to Studio when another PDF is opened later', async () => {
    vi.mocked(bridge.hasOpenedFiles).mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    render(<DesktopFileOpenHandler locale="en" />);
    await waitFor(() => expect(handler).toBeDefined());
    expect(push).not.toHaveBeenCalled();
    await act(async () => handler?.());
    await waitFor(() => expect(push).toHaveBeenCalledWith('/en/studio/'));
  });

  it('stays put when already in Studio (Studio opens the tabs itself)', async () => {
    pathname = '/en/studio/';
    vi.mocked(bridge.hasOpenedFiles).mockResolvedValue(true);
    render(<DesktopFileOpenHandler locale="en" />);
    await waitFor(() => expect(handler).toBeDefined());
    expect(push).not.toHaveBeenCalled();
  });
});
