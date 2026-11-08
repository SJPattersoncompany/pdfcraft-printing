/**
 * Software Updater & Version Comparison Tests
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  parseVersionParts,
  compareVersions,
  categorizeAsset,
  matchPlatformAssets,
  ignoreVersion,
  isVersionIgnored,
  snoozeUpdateInSession,
  isUpdateSnoozedInSession,
  saveUpdateSettings,
  shouldCheckUpdate,
  checkUpdate,
} from '@/lib/updater';
import type { ReleaseAsset } from '@/types/updater';

describe('Software Updater', () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
  });

  describe('parseVersionParts', () => {
    it('parses standard semver versions', () => {
      const parts = parseVersionParts('0.1.0');
      expect(parts.numbers).toEqual([0, 1, 0]);
      expect(parts.raw).toBe('0.1.0');
    });

    it('parses date-based tags with leading v and commit hashes', () => {
      const parts = parseVersionParts('v2026.09.12-6178132');
      expect(parts.numbers).toEqual([2026, 9, 12]);
      expect(parts.raw).toBe('2026.09.12-6178132');
    });
  });

  describe('compareVersions', () => {
    it('detects newer SemVer version', () => {
      expect(compareVersions('0.1.0', '0.2.0')).toBe(true);
      expect(compareVersions('1.0.0', '1.0.1')).toBe(true);
      expect(compareVersions('1.0.0', '1.0.0')).toBe(false);
      expect(compareVersions('1.2.0', '1.1.9')).toBe(false);
    });

    it('detects update when local version is default 0.1.0 and remote is date-tagged', () => {
      expect(compareVersions('0.1.0', 'v2026.09.12-6178132')).toBe(true);
      expect(compareVersions('0.0.0', 'v2026.09.12-6178132')).toBe(true);
    });

    it('detects newer date-tagged version across days', () => {
      expect(compareVersions('v2026.09.10-abc', 'v2026.09.12-def')).toBe(true);
      expect(compareVersions('v2026.09.15-abc', 'v2026.09.12-def')).toBe(false);
    });

    it('detects update when date is the same but commit hash differs (same-day releases)', () => {
      expect(compareVersions('v2026.09.12-1111111', 'v2026.09.12-2222222')).toBe(true);
    });

    it('returns false when version tags are identical', () => {
      expect(compareVersions('v2026.09.12-6178132', 'v2026.09.12-6178132')).toBe(false);
      expect(compareVersions('2026.09.12-6178132', 'v2026.09.12-6178132')).toBe(false);
    });

    it('handles falsy or empty versions gracefully', () => {
      expect(compareVersions('', 'v2026.09.12')).toBe(true);
      expect(compareVersions('v2026.09.12', '')).toBe(false);
    });
  });

  describe('categorizeAsset & matchPlatformAssets', () => {
    it('categorizes assets correctly', () => {
      expect(categorizeAsset('PDFCraft-Windows-x64-Portable.zip')).toBe('windows-portable');
      expect(categorizeAsset('PDFCraft-Windows-x64-Setup.exe')).toBe('windows-installer');
      expect(categorizeAsset('PDFCraft_0.1.0_x64_en-US.msi')).toBe('windows-installer');
      expect(categorizeAsset('PDFCraft.dmg')).toBe('macos-dmg');
      expect(categorizeAsset('PDFCraft.AppImage')).toBe('linux-appimage');
      expect(categorizeAsset('PDFCraft.deb')).toBe('linux-deb');
      expect(categorizeAsset('source-code.zip')).toBe('source');
    });

    it('matches Windows platform assets with portable prioritized', () => {
      const assets: ReleaseAsset[] = [
        {
          name: 'PDFCraft-Windows-x64-Portable.zip',
          downloadUrl: 'https://example.com/portable.zip',
          size: 1024,
          platformType: 'windows-portable',
          browserDownloadUrl: 'https://example.com/portable.zip',
        },
        {
          name: 'PDFCraft-Setup.exe',
          downloadUrl: 'https://example.com/setup.exe',
          size: 2048,
          platformType: 'windows-installer',
          browserDownloadUrl: 'https://example.com/setup.exe',
        },
      ];

      const matched = matchPlatformAssets(assets, 'windows');
      expect(matched.primary?.platformType).toBe('windows-portable');
      expect(matched.secondary?.platformType).toBe('windows-installer');
    });
  });

  describe('ignore and snooze functionality', () => {
    it('correctly tracks ignored versions in localStorage', () => {
      expect(isVersionIgnored('v2026.09.12')).toBe(false);
      ignoreVersion('v2026.09.12');
      expect(isVersionIgnored('v2026.09.12')).toBe(true);
      expect(isVersionIgnored('v2026.09.13')).toBe(false);
    });

    it('correctly tracks snoozed versions in sessionStorage', () => {
      expect(isUpdateSnoozedInSession('v2026.09.12')).toBe(false);
      snoozeUpdateInSession('v2026.09.12');
      expect(isUpdateSnoozedInSession('v2026.09.12')).toBe(true);
      expect(isUpdateSnoozedInSession('v2026.09.13')).toBe(false);
    });
  });

  describe('settings and checkUpdate', () => {
    it('checks shouldCheckUpdate frequency correctly', () => {
      expect(shouldCheckUpdate()).toBe(true);

      const now = Date.now();
      saveUpdateSettings({
        autoCheck: true,
        checkFrequencyHours: 24,
        lastCheckedTimestamp: now,
        ignoredVersions: [],
      });
      expect(shouldCheckUpdate()).toBe(false);

      saveUpdateSettings({
        autoCheck: true,
        checkFrequencyHours: 24,
        lastCheckedTimestamp: now - 25 * 3600 * 1000,
        ignoredVersions: [],
      });
      expect(shouldCheckUpdate()).toBe(true);
    });

    it('fetches release and discovers update via mocked fetch', async () => {
      const mockGitHubResponse = {
        tag_name: 'v2026.09.15-xyz',
        name: 'PDFCraft v2026.09.15',
        published_at: '2026-09-15T12:00:00Z',
        html_url: 'https://github.com/PDFCraftTool/pdfcraft/releases/tag/v2026.09.15-xyz',
        body: '## Release notes',
        assets: [
          {
            name: 'PDFCraft-Windows-x64-Portable.zip',
            browser_download_url: 'https://example.com/portable.zip',
            size: 10485760,
          },
        ],
      };

      global.fetch = vi.fn().mockResolvedValue({
        ok: true,
        json: async () => mockGitHubResponse,
      });

      const result = await checkUpdate({ force: true });
      expect(result.hasUpdate).toBe(true);
      expect(result.latestVersion).toBe('v2026.09.15-xyz');
      expect(result.matchedAssets.primary?.platformType).toBe('windows-portable');
    });

    it('disables update checking when NEXT_PUBLIC_DISABLE_UPDATE_CHECK is true', async () => {
      const originalEnv = process.env.NEXT_PUBLIC_DISABLE_UPDATE_CHECK;
      try {
        process.env.NEXT_PUBLIC_DISABLE_UPDATE_CHECK = 'true';
        expect(shouldCheckUpdate()).toBe(false);

        const result = await checkUpdate();
        expect(result.hasUpdate).toBe(false);
      } finally {
        if (originalEnv !== undefined) {
          process.env.NEXT_PUBLIC_DISABLE_UPDATE_CHECK = originalEnv;
        } else {
          delete process.env.NEXT_PUBLIC_DISABLE_UPDATE_CHECK;
        }
      }
    });

    it('does not query official releases when checks are disabled, even if forced on desktop', async () => {
      const originalEnv = process.env.NEXT_PUBLIC_DISABLE_UPDATE_CHECK;
      const fetchSpy = vi.fn();
      const previousFetch = global.fetch;
      try {
        process.env.NEXT_PUBLIC_DISABLE_UPDATE_CHECK = 'true';
        global.fetch = fetchSpy as unknown as typeof fetch;
        const result = await checkUpdate({ force: true, isDesktop: true });
        expect(result.hasUpdate).toBe(false);
        expect(result.latestVersion).toBe(result.currentVersion);
        expect(fetchSpy).not.toHaveBeenCalled();
      } finally {
        global.fetch = previousFetch;
        if (originalEnv !== undefined) {
          process.env.NEXT_PUBLIC_DISABLE_UPDATE_CHECK = originalEnv;
        } else {
          delete process.env.NEXT_PUBLIC_DISABLE_UPDATE_CHECK;
        }
      }
    });
  });
});
