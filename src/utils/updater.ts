import { invoke } from "@tauri-apps/api/core";
import {
  check as checkSignedUpdate,
  type DownloadEvent as OfficialDownloadEvent,
} from "@tauri-apps/plugin-updater";

export interface UpdateAsset {
  name: string;
  size: number;
  browser_download_url: string;
  /** 发布接口返回的 sha256 摘要（可能为空）；下载时会据此校验安装包。 */
  digest?: string | null;
}

export interface UpdateInfo {
  current_version: string;
  latest_version: string;
  notes: string;
  release_url: string;
  published_at: string;
  assets: UpdateAsset[];
  /** 桌面端由 Tauri 官方签名更新器提供；移动端仍使用发布资产列表。 */
  official?: boolean;
  /** 签名清单暂不可用时仅提供发布页入口，不执行应用内安装。 */
  manual_only?: boolean;
}

/**
 * 只暴露官方 updater 在页面需要的能力，避免把插件内部带私有字段的
 * Resource 类泄漏到 Vue 状态类型中。
 */
export interface OfficialUpdate {
  currentVersion: string;
  version: string;
  date?: string;
  body?: string;
  rawJson: Record<string, unknown>;
  download(onEvent?: (progress: OfficialDownloadEvent) => void): Promise<void>;
  install(options?: { restartAfterInstall?: boolean }): Promise<void>;
  close(): Promise<void>;
}

export type UpdatePlatform = "windows" | "macos" | "linux" | "android" | "ios";

/** 更新接口不可用（限流/网络拦截）时的兜底下载地址。 */
const DEFAULT_RELEASES_PAGE_URL = "https://github.com/yuchen0x1/Qzone-Archive/releases";
const configuredReleasesPageUrl = import.meta.env.VITE_RELEASES_URL?.trim();
export const RELEASES_PAGE_URL = configuredReleasesPageUrl || DEFAULT_RELEASES_PAGE_URL;

/** 国内网络访问 GitHub 受阻时的加速镜像前缀（社区维护，失效时可整体替换）。 */
const GITHUB_ACCELERATED_PREFIX = "https://ghfast.top/";

/** 为 GitHub 链接追加加速镜像前缀；非 github.com 链接原样返回。 */
export function acceleratedUrl(url: string): string {
  return /^https:\/\/github\.com\//.test(url)
    ? `${GITHUB_ACCELERATED_PREFIX}${url}`
    : url;
}

/** 查询发布仓库最新 Release；有新版本返回信息，已是最新返回 null。失败抛出错误文本。 */
export function checkForUpdate(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>("check_for_update");
}

/**
 * 桌面端使用 Tauri 官方签名更新器检查更新。
 * 公钥和 endpoint 由 tauri.conf.json 配置，校验失败时插件会拒绝返回可安装更新。
 */
export function checkOfficialForUpdate(): Promise<OfficialUpdate | null> {
  return checkSignedUpdate({ timeout: 20_000 });
}

/** 将官方更新对象映射到现有关于页展示模型；安装仍使用原始 Update 对象。 */
export function officialUpdateInfo(update: OfficialUpdate): UpdateInfo {
  return {
    current_version: update.currentVersion,
    latest_version: update.version,
    notes: update.body || "",
    release_url: RELEASES_PAGE_URL,
    published_at: update.date || "",
    assets: [],
    official: true,
    manual_only: false,
  };
}

export type { OfficialDownloadEvent };

export interface UpdateDownloadProgress {
  name: string;
  downloaded: number;
  total: number;
  percent: number;
}

export interface DownloadedUpdateAsset {
  path: string;
  name: string;
  size: number;
}

/** 应用内下载更新安装包（后端流式下载并通过 update-download-progress 事件上报进度）。
 *  传入所在资产的 digest 时后端会校验 SHA-256，不一致直接删除。 */
export function downloadUpdateAsset(url: string, digest?: string | null): Promise<DownloadedUpdateAsset> {
  return invoke<DownloadedUpdateAsset>("download_update_asset", { url, expectedDigest: digest || null });
}

export function cancelUpdateDownload(): Promise<void> {
  return invoke<void>("cancel_update_download");
}

/** 运行已下载的安装包：Windows 会先退出应用再静默安装并自动重启。 */
export function runUpdateInstaller(path: string): Promise<void> {
  return invoke<void>("run_update_installer", { path });
}

/** 是否支持应用内下载与一键安装（桌面平台）。 */
export function supportsInAppUpdate(currentPlatform: UpdatePlatform): boolean {
  return currentPlatform === "windows" || currentPlatform === "macos" || currentPlatform === "linux";
}

export function formatBytes(size: number): string {
  if (!size) return "未知大小";
  if (size >= 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`;
  if (size >= 1024) return `${(size / 1024).toFixed(0)} KB`;
  return `${size} B`;
}

export function isAndroidAsset(asset: UpdateAsset): boolean {
  return asset.name.includes("android");
}

export function isWindowsAsset(asset: UpdateAsset): boolean {
  return asset.name.endsWith("x64-setup.exe");
}

/** 按当前设备平台选择 Release 中最合适的安装包。 */
export function assetForPlatform(
  assets: UpdateAsset[],
  currentPlatform: UpdatePlatform,
  currentArch = "",
): UpdateAsset | undefined {
  const arch = currentArch.toLowerCase();
  const matches = {
    windows: (asset: UpdateAsset) => asset.name.endsWith("x64-setup.exe"),
    macos: (asset: UpdateAsset) => {
      if (!asset.name.endsWith(".dmg")) return false;
      const wantsArm = arch.includes("aarch") || arch.includes("arm");
      return wantsArm ? asset.name.includes("aarch64") : asset.name.includes("x64");
    },
    linux: (asset: UpdateAsset) => asset.name.endsWith(".AppImage"),
    android: (asset: UpdateAsset) => asset.name.toLowerCase().includes("android") && asset.name.endsWith(".apk"),
    ios: (asset: UpdateAsset) => asset.name.endsWith(".ipa"),
  } satisfies Record<UpdatePlatform, (asset: UpdateAsset) => boolean>;

  return assets.find(matches[currentPlatform]);
}

export function updateAssetLabel(name: string): string {
  if (name.endsWith(".exe")) return "Windows 安装包";
  if (name.endsWith(".apk")) return "Android 安装包";
  if (name.endsWith(".ipa")) return "iOS 安装包";
  if (name.endsWith(".dmg")) return "macOS 安装包";
  if (name.endsWith(".AppImage")) return "Linux AppImage";
  if (name.endsWith(".deb")) return "Debian/Ubuntu 安装包";
  if (name.endsWith(".rpm")) return "RPM 安装包";
  return name;
}
