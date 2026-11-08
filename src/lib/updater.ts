import {
  AssetPlatformType,
  HostPlatform,
  ReleaseAsset,
  ReleaseInfo,
  UpdateCheckResult,
  UpdateSettings,
} from '@/types/updater';

export const GITHUB_REPO = 'PDFCraftTool/pdfcraft';
export const DEFAULT_CURRENT_VERSION =
  process.env.NEXT_PUBLIC_APP_VERSION || '0.1.0';

const STORAGE_KEY = 'pdfcraft_update_settings';
const DEFAULT_CHECK_INTERVAL_HOURS = 24;

/**
 * Parses version numbers or tag strings into numeric components for comparison.
 * Handles both SemVer (e.g. 0.1.0, 1.2.3) and Date-based tags (e.g. 2026.09.10-abc, v2026.09.10).
 */
export function parseVersionParts(version: string): { numbers: number[]; raw: string } {
  const clean = version.trim().replace(/^v/i, '');
  // Extract major numeric segments (ignoring trailing git commit hashes)
  const baseVersion = clean.split('-')[0];
  const parts = baseVersion
    .split('.')
    .map((p) => {
      const n = parseInt(p, 10);
      return Number.isNaN(n) ? 0 : n;
    });

  return { numbers: parts, raw: clean };
}

/**
 * Compares two versions.
 * Returns `true` if `latestVersion` is strictly newer than `currentVersion`.
 * Handles both SemVer (0.1.0, 1.2.3) and Date-based tags (e.g. 2026.09.12-6178132, v2026.09.10).
 */
export function compareVersions(
  currentVersion: string,
  latestVersion: string,
  releasePublishedAt?: string
): boolean {
  if (!latestVersion) return false;
  if (!currentVersion) return true;

  const cur = parseVersionParts(currentVersion);
  const lat = parseVersionParts(latestVersion);

  if (cur.raw === lat.raw) {
    return false;
  }

  // Local development or initial fallback version
  if ((cur.raw === '0.1.0' || cur.raw === '0.0.0') && lat.raw !== cur.raw) {
    return true;
  }

  // Compare numerical segments
  const maxLen = Math.max(cur.numbers.length, lat.numbers.length);
  for (let i = 0; i < maxLen; i++) {
    const curNum = cur.numbers[i] || 0;
    const latNum = lat.numbers[i] || 0;
    if (latNum > curNum) return true;
    if (latNum < curNum) return false;
  }

  // If numerical segments match (e.g. both built on 2026.09.12) but raw tags differ:
  if (cur.raw !== lat.raw) {
    const buildDateStr = process.env.NEXT_PUBLIC_BUILD_DATE;
    if (releasePublishedAt && buildDateStr) {
      const releaseTime = new Date(releasePublishedAt).getTime();
      const buildTime = new Date(buildDateStr).getTime();
      if (!Number.isNaN(releaseTime) && !Number.isNaN(buildTime)) {
        return releaseTime > buildTime;
      }
    }
    // If we have different commit hashes on the same date and no verifiable timestamp,
    // treat differing build hashes as an update opportunity
    return true;
  }

  return false;
}

/**
 * Categorizes a release asset by filename.
 */
export function categorizeAsset(filename: string): AssetPlatformType {
  const lower = filename.toLowerCase();

  if (lower.includes('portable') && lower.endsWith('.zip')) {
    return 'windows-portable';
  }
  if (lower.endsWith('.msi') || (lower.endsWith('.exe') && !lower.includes('portable'))) {
    return 'windows-installer';
  }
  if (lower.endsWith('.dmg')) {
    return 'macos-dmg';
  }
  if (lower.endsWith('.appimage')) {
    return 'linux-appimage';
  }
  if (lower.endsWith('.deb')) {
    return 'linux-deb';
  }
  if (lower.endsWith('.zip') || lower.endsWith('.tar.gz')) {
    return 'source';
  }
  return 'other';
}

/**
 * Detects the client host platform.
 */
export function detectPlatform(): HostPlatform {
  if (typeof window === 'undefined') return 'unknown';

  const userAgent = (navigator.userAgent || '').toLowerCase();
  const platform = (navigator.platform || '').toLowerCase();

  if (userAgent.includes('win') || platform.includes('win')) {
    return 'windows';
  }
  if (userAgent.includes('mac') || platform.includes('mac')) {
    return 'macos';
  }
  if (userAgent.includes('linux') || platform.includes('linux')) {
    return 'linux';
  }
  return 'unknown';
}

/**
 * Matches assets according to the current client platform.
 */
export function matchPlatformAssets(
  assets: ReleaseAsset[],
  platform: HostPlatform
): {
  primary?: ReleaseAsset;
  secondary?: ReleaseAsset;
  all: ReleaseAsset[];
} {
  const validAssets = assets.filter(
    (a) => a.platformType !== 'source' && a.platformType !== 'other'
  );

  let primary: ReleaseAsset | undefined;
  let secondary: ReleaseAsset | undefined;

  switch (platform) {
    case 'windows':
      primary = validAssets.find((a) => a.platformType === 'windows-portable') ||
                validAssets.find((a) => a.platformType === 'windows-installer');
      secondary = validAssets.find(
        (a) =>
          a !== primary &&
          (a.platformType === 'windows-installer' || a.platformType === 'windows-portable')
      );
      break;
    case 'macos':
      primary = validAssets.find((a) => a.platformType === 'macos-dmg');
      break;
    case 'linux':
      primary = validAssets.find((a) => a.platformType === 'linux-appimage') ||
                validAssets.find((a) => a.platformType === 'linux-deb');
      secondary = validAssets.find(
        (a) =>
          a !== primary &&
          (a.platformType === 'linux-deb' || a.platformType === 'linux-appimage')
      );
      break;
    default:
      primary = validAssets[0];
  }

  return {
    primary,
    secondary,
    all: validAssets,
  };
}

/**
 * Retrieves update settings from localStorage.
 */
export function getUpdateSettings(): UpdateSettings {
  if (typeof window === 'undefined') {
    return {
      autoCheck: true,
      checkFrequencyHours: DEFAULT_CHECK_INTERVAL_HOURS,
      lastCheckedTimestamp: 0,
      ignoredVersions: [],
    };
  }

  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) {
      return {
        autoCheck: true,
        checkFrequencyHours: DEFAULT_CHECK_INTERVAL_HOURS,
        lastCheckedTimestamp: 0,
        ignoredVersions: [],
      };
    }
    const parsed = JSON.parse(raw);
    return {
      autoCheck: parsed.autoCheck ?? true,
      checkFrequencyHours: parsed.checkFrequencyHours ?? DEFAULT_CHECK_INTERVAL_HOURS,
      lastCheckedTimestamp: parsed.lastCheckedTimestamp ?? 0,
      ignoredVersions: Array.isArray(parsed.ignoredVersions) ? parsed.ignoredVersions : [],
    };
  } catch {
    return {
      autoCheck: true,
      checkFrequencyHours: DEFAULT_CHECK_INTERVAL_HOURS,
      lastCheckedTimestamp: 0,
      ignoredVersions: [],
    };
  }
}

/**
 * Saves update settings to localStorage.
 */
export function saveUpdateSettings(settings: UpdateSettings): void {
  if (typeof window === 'undefined') return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
  } catch {
    // ignore quota/storage errors
  }
}

/**
 * Checks whether it's time to run an automatic update check.
 */
export function shouldCheckUpdate(): boolean {
  if (process.env.NEXT_PUBLIC_DISABLE_UPDATE_CHECK === 'true') {
    return false;
  }
  const settings = getUpdateSettings();
  if (!settings.autoCheck) return false;

  const now = Date.now();
  const intervalMs = settings.checkFrequencyHours * 3600 * 1000;
  return now - settings.lastCheckedTimestamp >= intervalMs;
}

/**
 * Ignores a specific version so the user won't be prompted again.
 */
export function ignoreVersion(version: string): void {
  const settings = getUpdateSettings();
  if (!settings.ignoredVersions.includes(version)) {
    settings.ignoredVersions.push(version);
    saveUpdateSettings(settings);
  }
}

/**
 * Checks if a version is in the ignored list.
 */
export function isVersionIgnored(version: string): boolean {
  const settings = getUpdateSettings();
  return settings.ignoredVersions.includes(version);
}

const SESSION_SNOOZE_KEY = 'pdfcraft_update_snoozed_version';

/**
 * Checks if a specific version has been snoozed (reminded later) in the current browser/app session.
 */
export function isUpdateSnoozedInSession(version: string): boolean {
  if (typeof window === 'undefined') return false;
  try {
    const snoozed = sessionStorage.getItem(SESSION_SNOOZE_KEY);
    return snoozed === version;
  } catch {
    return false;
  }
}

/**
 * Snoozes update reminders for a specific version during the current session.
 */
export function snoozeUpdateInSession(version: string): void {
  if (typeof window === 'undefined') return;
  try {
    sessionStorage.setItem(SESSION_SNOOZE_KEY, version);
  } catch {
    // ignore quota/storage issues
  }
}

export const GITHUB_RELEASE_ENDPOINTS = [
  `https://api.github.com/repos/${GITHUB_REPO}/releases/latest`,
  `https://gh-proxy.com/https://api.github.com/repos/${GITHUB_REPO}/releases/latest`,
  `https://mirror.ghproxy.com/https://api.github.com/repos/${GITHUB_REPO}/releases/latest`,
];

/**
 * Fetches the latest release with multi-endpoint failover.
 * Tries official GitHub API first, falling back to mirror endpoints for global & domestic reliability.
 */
export async function fetchLatestRelease(): Promise<ReleaseInfo> {
  let lastError: Error | null = null;

  for (const endpoint of GITHUB_RELEASE_ENDPOINTS) {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 3500);

    try {
      const response = await fetch(endpoint, {
        headers: {
          Accept: 'application/vnd.github.v3+json',
        },
        signal: controller.signal,
      });

      if (!response.ok) {
        throw new Error(`API returned HTTP ${response.status}: ${response.statusText}`);
      }

      const data = await response.json();

      const assets: ReleaseAsset[] = Array.isArray(data.assets)
        ? data.assets.map((asset: { name: string; size: number; browser_download_url: string }) => ({
            name: asset.name,
            downloadUrl: asset.browser_download_url,
            size: asset.size,
            platformType: categorizeAsset(asset.name),
            browserDownloadUrl: asset.browser_download_url,
            mirrorDownloadUrl: `https://gh-proxy.com/${asset.browser_download_url}`,
          }))
        : [];

      return {
        tag: data.tag_name || '',
        name: data.name || data.tag_name || '',
        publishedAt: data.published_at || '',
        htmlUrl: data.html_url || `https://github.com/${GITHUB_REPO}/releases`,
        body: data.body || '',
        assets,
      };
    } catch (err) {
      lastError = err instanceof Error ? err : new Error(String(err));
      console.warn(`[Updater] Endpoint failed (${endpoint}):`, lastError.message);
    } finally {
      clearTimeout(timeoutId);
    }
  }

  throw lastError || new Error('Failed to fetch from all release endpoints');
}

export interface CheckUpdateOptions {
  force?: boolean;
  isDesktop?: boolean;
}

/**
 * Performs an in-app check for software updates.
 *
 * @param optionsOrForce If true or options.force is true, skips interval check and queries GitHub immediately.
 */
export async function checkUpdate(
  optionsOrForce: boolean | CheckUpdateOptions = false
): Promise<UpdateCheckResult> {
  const options: CheckUpdateOptions =
    typeof optionsOrForce === 'boolean'
      ? { force: optionsOrForce }
      : optionsOrForce;

  const currentVersion = DEFAULT_CURRENT_VERSION;
  const shouldBypassInterval = Boolean(options.force || options.isDesktop);

  // Hard off-switch. Desktop startup passes force:true, which used to ignore this
  // flag and still query PDFCraftTool/pdfcraft. Unofficial print-test installers
  // set the flag so they do not offer or download an official update.
  if (process.env.NEXT_PUBLIC_DISABLE_UPDATE_CHECK === 'true') {
    return {
      hasUpdate: false,
      currentVersion,
      latestVersion: currentVersion,
      matchedAssets: { all: [] },
    };
  }

  if (!shouldBypassInterval && !shouldCheckUpdate()) {
    return {
      hasUpdate: false,
      currentVersion,
      latestVersion: currentVersion,
      matchedAssets: { all: [] },
    };
  }

  try {
    const release = await fetchLatestRelease();
    const latestVersion = release.tag;
    const hasUpdate = compareVersions(currentVersion, latestVersion, release.publishedAt);

    // Record check timestamp
    const settings = getUpdateSettings();
    settings.lastCheckedTimestamp = Date.now();
    saveUpdateSettings(settings);

    const platform = detectPlatform();
    const matchedAssets = matchPlatformAssets(release.assets, platform);

    return {
      hasUpdate,
      currentVersion,
      latestVersion,
      release,
      matchedAssets,
    };
  } catch (error) {
    const rawError = error instanceof Error ? error.message : String(error);
    let friendlyError = rawError;
    if (rawError.includes('Failed to fetch') || rawError.includes('abort') || rawError.includes('NetworkError')) {
      friendlyError = `${rawError}: 无法连接至更新服务器（请检查网络连接、代理或客户端安全策略）`;
    }
    return {
      hasUpdate: false,
      currentVersion,
      latestVersion: currentVersion,
      matchedAssets: { all: [] },
      error: friendlyError,
    };
  }
}
