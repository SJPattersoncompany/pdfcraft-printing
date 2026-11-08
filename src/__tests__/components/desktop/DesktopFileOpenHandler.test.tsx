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
    vi.spyOn(window, 'confirm').mockReturnValue(true);
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

  it('does not leave a tool page when the user cancels', async () => {
    vi.mocked(window.confirm).mockReturnValue(false);
    vi.mocked(bridge.hasOpenedFiles).mockResolvedValue(true);
    render(<DesktopFileOpenHandler locale="en" />);
    await waitFor(() => expect(window.confirm).toHaveBeenCalled());
    expect(push).not.toHaveBeenCalled();
  });

  it('opens Studio from a non-tool page without a confirmation', async () => {
    pathname = '/en/';
    vi.mocked(bridge.hasOpenedFiles).mockResolvedValue(true);
    render(<DesktopFileOpenHandler locale="en" />);
    await waitFor(() => expect(push).toHaveBeenCalledWith('/en/studio/'));
    expect(window.confirm).not.toHaveBeenCalled();
  });

  it('stays put when already in Studio (Studio opens the tabs itself)', async () => {
    pathname = '/en/studio/';
    vi.mocked(bridge.hasOpenedFiles).mockResolvedValue(true);
    render(<DesktopFileOpenHandler locale="en" />);
    await waitFor(() => expect(handler).toBeDefined());
    expect(push).not.toHaveBeenCalled();
  });
});
