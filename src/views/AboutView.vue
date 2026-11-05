<script setup lang="ts">
import { onMounted, ref } from "vue";
import Button from "primevue/button";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  assetForPlatform,
  acceleratedUrl,
  checkForUpdate,
  formatBytes,
  RELEASES_PAGE_URL,
  type UpdateInfo,
} from "../utils/updater";
import { arch, platform } from "@tauri-apps/plugin-os";

const appVersion = ref("");
const updateChecking = ref(false);
const updateChecked = ref(false);
const updateInfo = ref<UpdateInfo | null>(null);
const updateError = ref("");

onMounted(async () => {
  try {
    appVersion.value = await getVersion();
  } catch {
    /* 读取失败不影响页面展示 */
  }
});

async function checkUpdate() {
  if (updateChecking.value) return;
  updateChecking.value = true;
  updateChecked.value = false;
  updateError.value = "";
  try {
    updateInfo.value = await checkForUpdate();
  } catch (reason) {
    updateInfo.value = null;
    updateError.value = String(reason);
  } finally {
    updateChecking.value = false;
    updateChecked.value = true;
  }
}

function currentUpdateAsset(info: UpdateInfo) {
  return assetForPlatform(info.assets, platform() as "windows" | "macos" | "linux" | "android" | "ios", arch());
}

interface ChangelogEntry {
  version: string;
  date: string;
  highlights: string[];
}

const CHANGELOG: ChangelogEntry[] = [
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
    date: "2026-11-04",
    highlights: [
      "留言板全量同步",
      "好友列表同步与联系人页",
      "JSON / Markdown 导出（含评论、回复与点赞名单）",
      "GitHub 首个正式 Release",
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
      <div class="settings-copy"><span class="settings-icon tone-blue"><i class="pi pi-book" /></span><div><h3>项目来源</h3><p>本工具源于开源的 QQ 空间归档项目（QzoneArchive），由现维护者在其基础上继续开发：新增在线更新检查、全平台自动发布与文档站点。</p></div></div>
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
              <small>{{ currentUpdateAsset(updateInfo) ? `${currentUpdateAsset(updateInfo)!.name} · ${formatBytes(currentUpdateAsset(updateInfo)!.size)}` : "请在发布页手动下载" }}</small>
            </div>
          </div>
          <div class="about-links">
            <Button v-if="currentUpdateAsset(updateInfo)" label="下载对应安装包" icon="pi pi-download" @click="openUrl(currentUpdateAsset(updateInfo)!.browser_download_url)" />
            <Button label="发布页" icon="pi pi-external-link" severity="secondary" outlined @click="openUrl(updateInfo.release_url)" />
            <Button v-if="acceleratedUrl(updateInfo.release_url) !== updateInfo.release_url" label="加速打开" icon="pi pi-bolt" severity="secondary" outlined @click="openUrl(acceleratedUrl(updateInfo.release_url))" />
          </div>
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
