<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import Button from "primevue/button";
import ProgressBar from "primevue/progressbar";
import { getVersion } from "@tauri-apps/api/app";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { relaunch } from "@tauri-apps/plugin-process";
import {
  assetForPlatform,
  acceleratedUrl,
  cancelUpdateDownload,
  checkForUpdate,
  checkOfficialForUpdate,
  downloadUpdateAsset,
  formatBytes,
  officialUpdateInfo,
  runUpdateInstaller,
  supportsInAppUpdate,
  RELEASES_PAGE_URL,
  type DownloadedUpdateAsset,
  type UpdateDownloadProgress,
  type UpdateInfo,
  type UpdatePlatform,
  type OfficialDownloadEvent,
  type OfficialUpdate,
} from "../utils/updater";
import { arch, platform } from "@tauri-apps/plugin-os";

const appVersion = ref("");
const updateChecking = ref(false);
const updateChecked = ref(false);
const updateInfo = ref<UpdateInfo | null>(null);
const updateError = ref("");
const officialUpdate = ref<OfficialUpdate | null>(null);

const currentPlatform = platform() as UpdatePlatform;
const canUpdateInApp = supportsInAppUpdate(currentPlatform);

type DownloadState = "idle" | "downloading" | "done" | "error";
const downloadState = ref<DownloadState>("idle");
const downloadPercent = ref(0);
const downloadReceived = ref(0);
const downloadTotal = ref(0);
const downloadError = ref("");
const downloadedUpdate = ref<DownloadedUpdateAsset>();
const installing = ref(false);
let unlistenProgress: UnlistenFn | undefined;

onMounted(async () => {
  try {
    appVersion.value = await getVersion();
  } catch {
    /* 读取失败不影响页面展示 */
  }
  unlistenProgress = await listen<UpdateDownloadProgress>("update-download-progress", (event) => {
    const payload = event.payload;
    downloadPercent.value = payload.percent;
    downloadReceived.value = payload.downloaded;
    downloadTotal.value = payload.total;
  });
});

onBeforeUnmount(() => {
  unlistenProgress?.();
  void closeOfficialUpdate();
});

async function checkUpdate() {
  if (updateChecking.value) return;
  updateChecking.value = true;
  updateChecked.value = false;
  updateError.value = "";
  resetDownload();
  await closeOfficialUpdate();
  try {
    if (canUpdateInApp) {
      // 桌面端只接受 Tauri 官方签名更新器的结果，避免把未签名 Release
      // 资产误当成可安装更新。旧 Release 可能还没有 latest.json，
      // 此时仅回退到 GitHub API 读取版本并打开发布页，不执行应用内安装。
      try {
        officialUpdate.value = await checkOfficialForUpdate();
        updateInfo.value = officialUpdate.value ? officialUpdateInfo(officialUpdate.value) : null;
      } catch {
        const fallback = await checkForUpdate();
        updateInfo.value = fallback ? { ...fallback, manual_only: true } : null;
      }
    } else {
      updateInfo.value = await checkForUpdate();
    }
  } catch {
    updateInfo.value = null;
    // 不把 Tauri/网络内部错误直接展示给用户，保留可操作的发布页入口。
    updateError.value = "暂时无法检查更新，请稍后重试或打开发布页。";
  } finally {
    updateChecking.value = false;
    updateChecked.value = true;
  }
}

async function closeOfficialUpdate() {
  if (!officialUpdate.value) return;
  try {
    await officialUpdate.value.close();
  } catch {
    /* 更新资源释放失败不影响页面继续使用 */
  }
  officialUpdate.value = null;
}

function resetDownload() {
  downloadState.value = "idle";
  downloadPercent.value = 0;
  downloadReceived.value = 0;
  downloadTotal.value = 0;
  downloadError.value = "";
  downloadedUpdate.value = undefined;
  installing.value = false;
}

function currentUpdateAsset(info: UpdateInfo) {
  return assetForPlatform(info.assets, platform() as "windows" | "macos" | "linux" | "android" | "ios", arch());
}

/** 桌面平台走应用内下载；移动端仍交给系统浏览器下载安装包。 */
async function startUpdate(info: UpdateInfo) {
  if (info.official && officialUpdate.value) {
    await startOfficialUpdate(officialUpdate.value);
    return;
  }
  if (info.manual_only) {
    await openUrl(info.release_url);
    return;
  }
  const asset = currentUpdateAsset(info);
  if (!asset) return;
  if (!canUpdateInApp) {
    await openUrl(asset.browser_download_url);
    return;
  }
  downloadState.value = "downloading";
  downloadPercent.value = 0;
  downloadReceived.value = 0;
  downloadTotal.value = asset.size;
  downloadError.value = "";
  try {
    const urls = [asset.browser_download_url];
    const accelerated = acceleratedUrl(asset.browser_download_url);
    if (accelerated !== asset.browser_download_url) urls.push(accelerated);
    let lastError = "";
    for (const url of urls) {
      try {
        downloadedUpdate.value = await downloadUpdateAsset(url, asset.digest);
        downloadState.value = "done";
        downloadPercent.value = 100;
        return;
      } catch (reason) {
        lastError = String(reason);
        if (lastError.includes("取消")) break;
        resetProgressForRetry();
      }
    }
    throw new Error(lastError);
  } catch (reason) {
    downloadState.value = "error";
    downloadError.value = String(reason);
  }
}

async function startOfficialUpdate(update: OfficialUpdate) {
  downloadState.value = "downloading";
  downloadPercent.value = 0;
  downloadReceived.value = 0;
  downloadTotal.value = 0;
  downloadError.value = "";
  try {
    await update.download((event: OfficialDownloadEvent) => {
      if (event.event === "Started") {
        downloadTotal.value = event.data.contentLength ?? 0;
      } else if (event.event === "Progress") {
        downloadReceived.value += event.data.chunkLength;
        downloadPercent.value = downloadTotal.value
          ? Math.min(100, (downloadReceived.value / downloadTotal.value) * 100)
          : 0;
      } else {
        downloadPercent.value = 100;
      }
    });
    downloadedUpdate.value = {
      path: "",
      name: `QzoneArchive_${update.version}`,
      size: downloadReceived.value,
    };
    downloadState.value = "done";
    downloadPercent.value = 100;
  } catch (reason) {
    downloadState.value = "error";
    downloadError.value = String(reason);
  }
}

function resetProgressForRetry() {
  downloadPercent.value = 0;
  downloadReceived.value = 0;
}

async function cancelDownload() {
  try {
    await cancelUpdateDownload();
  } catch {
    /* 取消失败时下载会自行超时结束 */
  }
}

async function installUpdate() {
  if (!downloadedUpdate.value || installing.value) return;
  installing.value = true;
  downloadError.value = "";
  try {
    if (updateInfo.value?.official && officialUpdate.value) {
      await officialUpdate.value.install({ restartAfterInstall: currentPlatform === "windows" });
      // 官方插件在 Windows 安装器启动后退出应用；macOS/Linux 需要显式重启。
      if (currentPlatform !== "windows") await relaunch();
    } else {
      await runUpdateInstaller(downloadedUpdate.value.path);
      // 自定义移动/旧桌面路径：Windows 下应用随即退出；其余平台弹出系统安装界面或目录。
    }
  } catch (reason) {
    installing.value = false;
    downloadError.value = String(reason);
  }
}

function installButtonLabel() {
  return currentPlatform === "windows" ? "立即安装并重启" : "立即安装";
}

interface ChangelogEntry {
  version: string;
  date: string;
  highlights: string[];
}

const CHANGELOG: ChangelogEntry[] = [
  {
    version: "1.3.5",
    date: "2026-11-12",
    highlights: [
      "修复 Windows 桌面和开始菜单快捷方式的白纸图标",
      "修正更新签名公钥，与发布签名密钥保持一致",
      "最后发布版本：Windows x64 安装包与签名更新文件",
    ],
  },
  {
    version: "1.3.4",
    date: "2026-11-12",
    highlights: [
      "修复 Windows 更新清单缺失时的检查提示，改为安全回退到发布页",
      "发布流程收敛为 Windows NSIS，并生成 Tauri 签名更新清单",
      "提升备份、搜索、相册进度和媒体缓存的稳定性",
    ],
  },
  {
    version: "1.3.3",
    date: "2026-11-08",
    highlights: [
      "顶部新增全局搜索：动态、评论、相册、照片与好友统一检索",
      "离线包导出支持实时进度、随时取消与按文件增量导出",
      "新增完整备份/恢复：恢复前完整性校验，失败自动回滚",
      "暗色模式刷新后保持，设置页新增自动检查更新与媒体按需加载开关",
      "修复 GLib 依赖安全问题与若干依赖漏洞",
    ],
  },
  {
    version: "1.3.2",
    date: "2026-11-06",
    highlights: [
      "软件更新支持应用内下载：带实时进度条与取消操作",
      "新增「立即安装并重启」：Windows 静默安装后自动重启应用",
      "下载失败自动回退加速镜像重试",
    ],
  },
  {
    version: "1.3.1",
    date: "2026-11-06",
    highlights: ["关于页新增更新日志时间线"],
  },
  {
    version: "1.3.0",
    date: "2026-11-06",
    highlights: [
      "全新科技感界面：玻璃拟态卡片、全息光效与呼吸灯动效",
      "官方说说主页全量同步，无点赞/评论的说说也能归档",
      "首页新增「那年今日」时光记忆卡片",
      "新增一键隐私保护模式，全局脱敏模糊",
      "归档照片写入真实拍摄时间，SQLite 启用 WAL 提速",
      "新增 macOS / Linux / Android / iOS 官方构建产物",
    ],
  },
  {
    version: "1.2.0",
    date: "2026-09-06",
    highlights: [
      "留言板全量同步",
      "好友列表同步与联系人页",
      "JSON / Markdown 导出（含评论、回复与点赞名单）",
      "源码标签发布于 9 月 6 日，GitHub 首个 Release 发布于 11 月 5 日",
    ],
  },
  {
    version: "1.1.0",
    date: "2026-09-06",
    highlights: ["相册备份与断点续传", "离线导出包（HTML + 媒体 + data.json）"],
  },
  {
    version: "1.0.0",
    date: "2026-09-05",
    highlights: ["首个公开版本：扫码登录、动态归档、本地浏览"],
  },
];

function isCurrentVersion(version: string) {
  const current = appVersion.value.replace(/^v/i, "");
  return Boolean(current) && current === version;
}
</script>

<template>
  <section class="settings-stack about-stack">
    <header class="about-header">
      <div>
        <h1>关于</h1>
        <p>空间归档 · 本地 QQ 空间归档工具 <span class="version-badge">v{{ appVersion || "未知" }}</span></p>
      </div>
    </header>

    <article class="surface-card settings-card">
      <div class="settings-copy"><span class="settings-icon tone-blue"><i class="pi pi-book" /></span><div><h3>项目来源与状态</h3><p>本工具源于开源的 QQ 空间归档项目（QzoneArchive）。项目于 2026-11-13 停止维护并转为存档，最后发布版本为 v1.3.5；历史源码和下载保留，不再提供功能更新或安全修复。</p></div></div>
    </article>

    <article class="surface-card settings-card">
      <div class="settings-copy"><span class="settings-icon tone-green"><i class="pi pi-user" /></span><div><h3>原作者与原始仓库</h3><p>LibraHp_0928 · Gaoshu705/QzoneArchive</p></div></div>
      <div class="about-links">
        <Button label="原项目仓库" icon="pi pi-github" severity="secondary" outlined @click="openUrl('https://github.com/Gaoshu705/QzoneArchive')" />
        <Button label="B 站主页" icon="pi pi-external-link" severity="secondary" text @click="openUrl('https://space.bilibili.com/1117414477')" />
      </div>
    </article>

    <article class="surface-card settings-card">
      <div class="settings-copy"><span class="settings-icon tone-purple"><i class="pi pi-heart-fill" /></span><div><h3>项目状态</h3><p>正常更新中 · yuchen0x1 · Qzone-Archive</p></div></div>
      <div class="about-links">
      <Button label="项目主页" icon="pi pi-github" severity="secondary" outlined @click="openUrl('https://github.com/yuchen0x1/Qzone-Archive')" />
      </div>
    </article>

    <article class="surface-card settings-card about-card">
      <div class="about-main">
        <div class="settings-copy"><span class="settings-icon"><i class="pi pi-refresh" /></span><div><h3>软件更新</h3><p>检查配置的发布仓库是否有新版本发布。</p></div></div>
        <Button :label="updateChecking ? '检查中…' : '检查更新'" icon="pi pi-refresh" :loading="updateChecking" severity="secondary" outlined @click="checkUpdate" />
      </div>
      <div class="update-section">
        <div class="update-row">
          <div class="update-text">
            <p v-if="!updateInfo && !updateError && !updateChecking && !updateChecked">当前没有正在进行的检查。</p>
            <p v-else-if="updateError" class="archive-error"><i class="pi pi-exclamation-circle" />{{ updateError }}</p>
            <p v-else-if="updateInfo">发现新版本 v{{ updateInfo.latest_version }}（当前 v{{ updateInfo.current_version }}）。</p>
            <p v-else>当前已是最新版本。</p>
          </div>
        </div>
        <div v-if="updateError" class="update-available">
          <div class="update-meta">
            <i class="pi pi-globe" />
            <div>
              <strong>发布源暂时无法访问</strong>
              <small>可以直接打开发布页，或使用构建时配置的其他发布源。</small>
            </div>
          </div>
          <div class="about-links">
            <Button label="发布页" icon="pi pi-external-link" @click="openUrl(RELEASES_PAGE_URL)" />
            <Button v-if="acceleratedUrl(RELEASES_PAGE_URL) !== RELEASES_PAGE_URL" label="加速打开" icon="pi pi-bolt" severity="secondary" outlined @click="openUrl(acceleratedUrl(RELEASES_PAGE_URL))" />
          </div>
        </div>
        <div v-if="updateInfo" class="update-available">
          <div class="update-meta">
            <i class="pi pi-download" />
            <div>
              <strong>v{{ updateInfo.latest_version }}{{ updateInfo.published_at && !Number.isNaN(new Date(updateInfo.published_at).getTime()) ? ` · ${new Date(updateInfo.published_at).toLocaleDateString()}` : "" }}</strong>
            <small>{{ updateInfo.official ? "Tauri 官方签名更新包" : updateInfo.manual_only ? "签名清单暂不可用，请在发布页手动下载" : currentUpdateAsset(updateInfo) ? `${currentUpdateAsset(updateInfo)!.name} · ${formatBytes(currentUpdateAsset(updateInfo)!.size)}` : "请在发布页手动下载" }}</small>
            </div>
          </div>

          <div v-if="downloadState === 'downloading' || downloadState === 'done'" class="update-download">
            <ProgressBar :value="downloadState === 'done' ? 100 : downloadPercent" :show-value="false" :class="{ 'is-done': downloadState === 'done' }" style="height: 7px" />
            <div class="update-download-row">
              <small v-if="downloadState === 'downloading'">
                {{ canUpdateInApp ? "正在下载更新" : "正在下载安装包" }} {{ downloadPercent.toFixed(1) }}% · {{ formatBytes(downloadReceived) }}<template v-if="downloadTotal"> / {{ formatBytes(downloadTotal) }}</template>
              </small>
              <small v-else>
                <i class="pi pi-check-circle" /> 下载完成 · {{ formatBytes(downloadedUpdate?.size ?? 0) }}{{ currentPlatform === "windows" ? "，安装后应用会自动重启" : "" }}
              </small>
              <Button v-if="downloadState === 'downloading' && !updateInfo.official" label="取消下载" size="small" severity="secondary" text @click="cancelDownload" />
              <Button v-if="downloadState === 'done'" :label="installing ? '正在启动安装…' : installButtonLabel()" icon="pi pi-play" size="small" :loading="installing" @click="installUpdate" />
            </div>
          </div>

          <p v-if="updateInfo.notes" class="update-notes">{{ updateInfo.notes }}</p>

          <div v-if="downloadState !== 'downloading' && downloadState !== 'done'" class="about-links">
            <Button v-if="updateInfo.official || (!canUpdateInApp && currentUpdateAsset(updateInfo))" :label="canUpdateInApp ? '下载更新' : '下载安装包'" icon="pi pi-download" @click="startUpdate(updateInfo)" />
            <Button label="发布页" icon="pi pi-external-link" severity="secondary" outlined @click="openUrl(updateInfo.release_url)" />
            <Button v-if="acceleratedUrl(updateInfo.release_url) !== updateInfo.release_url" label="加速打开" icon="pi pi-bolt" severity="secondary" outlined @click="openUrl(acceleratedUrl(updateInfo.release_url))" />
          </div>

          <p v-if="downloadError" class="update-download-error"><i class="pi pi-exclamation-circle" />{{ downloadError }}</p>
        </div>
      </div>
    </article>

    <article class="surface-card changelog-card">
      <div class="changelog-head">
        <div class="settings-copy"><span class="settings-icon tone-teal"><i class="pi pi-history" /></span><div><h3>更新日志</h3><p>各版本的主要变化，完整说明见发布页。</p></div></div>
        <Button label="完整更新日志" icon="pi pi-external-link" severity="secondary" text @click="openUrl(RELEASES_PAGE_URL)" />
      </div>
      <ol class="changelog-timeline">
        <li v-for="entry in CHANGELOG" :key="entry.version" :class="{ 'is-current': isCurrentVersion(entry.version) }">
          <span class="changelog-dot" />
          <div class="changelog-entry">
            <div class="changelog-entry-head">
              <span class="changelog-version">v{{ entry.version }}</span>
              <span v-if="isCurrentVersion(entry.version)" class="changelog-current-badge">当前版本</span>
              <time>{{ entry.date }}</time>
            </div>
            <ul class="changelog-highlights">
              <li v-for="item in entry.highlights" :key="item">{{ item }}</li>
            </ul>
          </div>
        </li>
      </ol>
    </article>
  </section>
</template>
