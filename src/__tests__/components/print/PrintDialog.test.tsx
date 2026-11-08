import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup, act } from '@testing-library/react';
import React from 'react';
import { PrintDialog, SELECTED_PRINTER_STORAGE_KEY } from '@/components/print/PrintDialog';
import * as tauriBridge from '@/lib/tauri-bridge';

vi.mock('next-intl', () => ({
  useLocale: () => 'en',
  useTranslations: () => (key: string) => key,
}));

const printers = [
  { name: 'Brother HL-L2350DW', is_default: true, port_name: null },
  { name: 'Microsoft Print to PDF', is_default: false, port_name: null },
];

describe('PrintDialog', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });
  afterEach(() => cleanup());

  it('lists printers with the default first and selected', async () => {
    vi.spyOn(tauriBridge, 'listSystemPrinters').mockResolvedValue(printers);
    render(<PrintDialog isOpen onClose={() => {}} file={new Blob(['%PDF'])} />);
    const select = (await screen.findByRole('combobox')) as HTMLSelectElement;
    expect(select.value).toBe('Brother HL-L2350DW');
    expect(screen.getByText('Brother HL-L2350DW (default)')).toBeInTheDocument();
  });

  it('prints to the chosen printer with copies, duplex and the document name', async () => {
    vi.spyOn(tauriBridge, 'listSystemPrinters').mockResolvedValue(printers);
    const printSpy = vi.spyOn(tauriBridge, 'printPdfFile').mockResolvedValue({ success: true });
    const file = new Blob(['%PDF']);
    render(<PrintDialog isOpen onClose={() => {}} file={file} documentName="123 Main St.pdf" />);

    await screen.findByRole('combobox');
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '2' } });
    fireEvent.click(screen.getByLabelText('Print on both sides'));
    fireEvent.click(screen.getByRole('button', { name: 'Print' }));

    await waitFor(() =>
      expect(printSpy).toHaveBeenCalledWith(file, 'Brother HL-L2350DW', {
        copies: 2,
        duplex: true,
        jobName: '123 Main St.pdf',
      })
    );
    expect(await screen.findByText('Sent to "Brother HL-L2350DW"')).toBeInTheDocument();
    expect(localStorage.getItem(SELECTED_PRINTER_STORAGE_KEY)).toBe('Brother HL-L2350DW');
  });

  it('shows the printer error instead of closing', async () => {
    vi.spyOn(tauriBridge, 'listSystemPrinters').mockResolvedValue(printers);
    vi.spyOn(tauriBridge, 'printPdfFile').mockResolvedValue({
      success: false,
      error: 'Printer "Brother HL-L2350DW" did not accept the job. It may be offline or paused.',
    });
    render(<PrintDialog isOpen onClose={() => {}} file={new Blob(['%PDF'])} />);
    await screen.findByRole('combobox');
    fireEvent.click(screen.getByRole('button', { name: 'Print' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('may be offline or paused');
  });

  it('offers Add a printer when none are found and refreshes after returning', async () => {
    const listSpy = vi
      .spyOn(tauriBridge, 'listSystemPrinters')
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce(printers);
    const openSpy = vi.spyOn(tauriBridge, 'openPrinterSettings').mockResolvedValue({ success: true });

    render(<PrintDialog isOpen onClose={() => {}} file={new Blob(['%PDF'])} />);
    expect(await screen.findByText('No printers found')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /Add a printer/ }));
    await waitFor(() => expect(openSpy).toHaveBeenCalledWith('add'));

    // User comes back to the app after finishing the Windows wizard.
    act(() => {
      window.dispatchEvent(new Event('focus'));
    });
    expect(await screen.findByRole('combobox')).toBeInTheDocument();
    expect(listSpy).toHaveBeenCalledTimes(2);
  });

  it('shows the reason when the printer list cannot be read', async () => {
    vi.spyOn(tauriBridge, 'listSystemPrinters').mockRejectedValue(new Error('Print Spooler service is not running'));
    render(<PrintDialog isOpen onClose={() => {}} file={new Blob(['%PDF'])} />);
    expect(await screen.findByText('Could not read the printer list')).toBeInTheDocument();
    expect(screen.getByText('Print Spooler service is not running')).toBeInTheDocument();
  });
});
