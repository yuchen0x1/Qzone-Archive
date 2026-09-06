<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { readFile, writeFile } from "@tauri-apps/plugin-fs";
import Button from "primevue/button";
import Dialog from "primevue/dialog";
import ProgressBar from "primevue/progressbar";
import Tag from "primevue/tag";
import { loadRemoteImageBlob } from "../utils/archiveImage";
import { getArchiveInterval } from "../utils/appSettings";
import { useAuthStore } from "../stores/auth";
import {
  cancelAlbumBackup, deleteArchivedAlbum, getAlbumProgress, listArchivedAlbums,
  listArchivedAlbumPhotos, listRemoteAlbums, loadAlbumPhoto, resetFailedAlbumPhotos,
  startAlbumBackup, type AlbumPhoto, type AlbumProgress, type AlbumSummary,
} from "../utils/qzone";

const PHOTO_PAGE_SIZE = 60;
const authStore = useAuthStore();
const albums = ref<AlbumSummary[]>([]);
const loading = ref(false);
const offlineNotice = ref("");
const error = ref("");
const starting = ref(false);
const progress = ref<AlbumProgress>({ status: "idle", message: "尚未开始相册备份", albumsTotal: 0, albumsDone: 0, photosTotal: 0, photosDone: 0, photosFailed: 0 });
const currentTime = ref(Date.now());
const coverSources = reactive<Record<string, string>>({});
const coverLoading = reactive<Record<string, boolean>>({});
const confirmVisible = ref(false);
const deleting = ref(false);
const pendingDelete = ref<AlbumSummary>();

const photoDialogVisible = ref(false);
const photoAlbum = ref<AlbumSummary>();
const photos = ref<AlbumPhoto[]>([]);
const photoTotal = ref(0);
const photoLoading = ref(false);
const photoError = ref("");
const photoSources = reactive<Record<number, string>>({});
const photoLoadingMap = reactive<Record<number, boolean>>({});
const photoErrors = reactive<Record<number, string>>({});
const previewVisible = ref(false);
const previewLoading = ref(false);
const previewSrc = ref("");
const previewPhoto = ref<AlbumPhoto>();
const savingPreview = ref(false);
let timer: ReturnType<typeof setInterval> | undefined;
let imageObserver: IntersectionObserver | undefined;

const running = computed(() => progress.value.status === "running");
const rateLimited = computed(() => progress.value.status === "limited");
const rateWaiting = computed(() => Boolean(progress.value.retryAt && progress.value.retryAt * 1000 > currentTime.value));
const remainingText = computed(() => {
  const retryAt = progress.value.retryAt;
  if (!retryAt) return "";
  const seconds = Math.max(0, Math.ceil(retryAt - currentTime.value / 1000));
  const minutes = Math.floor(seconds / 60);
  return minutes > 0 ? `${minutes} 分 ${seconds % 60} 秒` : `${seconds} 秒`;
});
const progressPercent = computed(() => progress.value.photosTotal > 0 ? Math.min(100, (progress.value.photosDone / progress.value.photosTotal) * 100) : 0);
const statusSeverity = computed(() => {
  switch (progress.value.status) {
    case "running": return "info";
    case "completed": return "success";
    case "limited": return "warn";
    case "error": return "danger";
    default: return "secondary";
  }
});
const statusText = computed(() => {
  switch (progress.value.status) {
    case "running": return "备份中";
    case "completed": return "已完成";
    case "limited": return "频率保护";
    case "error": return "失败";
    case "cancelled": return "已取消";
    default: return "待开始";
  }
});
const hasBackedUp = computed(() => albums.value.some((album) => album.backedCount > 0));
const allPhotosLoaded = computed(() => photos.value.length >= photoTotal.value);
const formatTime = (seconds?: number) => seconds ? new Intl.DateTimeFormat("zh-CN", { dateStyle: "medium" }).format(new Date(seconds * 1000)) : "时间未知";
const albumStatusText = (album: AlbumSummary) => {
  if (album.failedCount > 0) return { label: `失败 ${album.failedCount} 张`, severity: "warn" as const };
  if (album.localStatus === "completed" && album.backedCount > 0) return { label: "已备份", severity: "success" as const };
  if (album.localStatus === "partial") return { label: "部分备份", severity: "warn" as const };
  if (album.backedCount > 0) return { label: `已备份 ${album.backedCount} 张`, severity: "success" as const };
  return { label: "未备份", severity: "secondary" as const };
};

async function refreshAlbums() {
  if (!authStore.loggedIn) return;
  loading.value = true; error.value = ""; offlineNotice.value = "";
  try {
    albums.value = await listRemoteAlbums();
    await nextTick();
    observeCovers();
  } catch (reason) {
    try {
      albums.value = await listArchivedAlbums();
      offlineNotice.value = `无法连接 QQ 空间，已显示本地已备份的相册（${String(reason)}）`;
      await nextTick();
      observeCovers();
    } catch (fallbackReason) { error.value = `读取相册失败：${String(fallbackReason)}`; }
  } finally { loading.value = false; }
}

async function refreshProgress() {
  try { progress.value = await getAlbumProgress(); } catch { /* 保留当前状态 */ }
}

function beginPolling() {
  if (timer) return;
  timer = setInterval(async () => {
    currentTime.value = Date.now();
    await refreshProgress();
    if (progress.value.status !== "running") {
      window.clearInterval(timer);
      timer = undefined;
      await refreshAlbums();
    }
  }, 600);
}

async function startBackup(albumIds?: string[]) {
  if (starting.value || running.value || !authStore.loggedIn) return;
  starting.value = true; error.value = "";
  try {
    progress.value = await startAlbumBackup(getArchiveInterval(), albumIds);
    beginPolling();
    currentTime.value = Date.now();
  } catch (reason) { error.value = `启动相册备份失败：${String(reason)}`; }
  finally { starting.value = false; }
}

async function stopBackup() {
  try { await cancelAlbumBackup(); } catch (reason) { error.value = String(reason); }
}

function observeCovers() {
  const observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      const element = entry.target as HTMLElement;
      const albumId = element.dataset.albumCover;
      if (albumId) void loadCover(albumId);
      observer.unobserve(element);
    }
  }, { rootMargin: "240px 0px" });
  document.querySelectorAll<HTMLElement>(".albums-page [data-album-cover]").forEach((element) => observer.observe(element));
}

async function loadCover(albumId: string) {
  const album = albums.value.find((item) => item.albumId === albumId);
  if (!album || coverSources[albumId] || coverLoading[albumId] || !album.coverUrl) return;
  coverLoading[albumId] = true;
  try { coverSources[albumId] = await loadRemoteImageBlob(album.coverUrl); }
  catch { /* 封面失败显示占位图标 */ }
  finally { delete coverLoading[albumId]; }
}

async function openPhotos(album: AlbumSummary) {
  photoAlbum.value = album;
  photoDialogVisible.value = true;
  photoError.value = "";
  if (photos.value[0]?.albumId !== album.albumId) {
    photos.value = [];
    photoTotal.value = 0;
    Object.keys(photoSources).forEach((key) => delete photoSources[Number(key)]);
    Object.keys(photoErrors).forEach((key) => delete photoErrors[Number(key)]);
  }
  await loadPhotos(true);
}

async function loadPhotos(reset = false) {
  const album = photoAlbum.value;
  if (!album || photoLoading.value || (!reset && allPhotosLoaded.value)) return;
  photoLoading.value = true; photoError.value = "";
  try {
    const page = await listArchivedAlbumPhotos(album.albumId, PHOTO_PAGE_SIZE, reset ? 0 : photos.value.length);
    photoTotal.value = page.total;
    photos.value = reset ? page.items : [...photos.value, ...page.items];
    await nextTick();
    observePhotos();
  } catch (reason) { photoError.value = `读取照片失败：${String(reason)}`; }
  finally { photoLoading.value = false; }
}

function observePhotos() {
  imageObserver?.disconnect();
  imageObserver = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      const element = entry.target as HTMLElement;
      const photoId = Number(element.dataset.albumPhoto);
      if (Number.isFinite(photoId)) void loadPhotoImage(photoId);
      imageObserver?.unobserve(element);
    }
  }, { rootMargin: "320px 0px" });
  document.querySelectorAll<HTMLElement>(".albums-photo-grid [data-album-photo]").forEach((element) => imageObserver?.observe(element));
}

async function loadPhotoImage(photoId: number) {
  if (photoSources[photoId] || photoLoadingMap[photoId]) return;
  const photo = photos.value.find((item) => item.id === photoId);
  if (!photo) return;
  photoLoadingMap[photoId] = true;
  delete photoErrors[photoId];
  try {
    if (photo.status === "done" && photo.localPath) {
      photoSources[photoId] = convertFileSrc(photo.localPath);
      return;
    }
    const url = photo.previewUrl || photo.url;
    if (!url) throw new Error("没有可用的照片地址");
    photoSources[photoId] = await loadRemoteImageBlob(url);
  } catch (reason) { photoErrors[photoId] = String(reason); }
  finally { delete photoLoadingMap[photoId]; }
}

async function retryPhotoImage(photoId: number) {
  delete photoSources[photoId];
  await loadPhotoImage(photoId);
}

async function openPreview(photo: AlbumPhoto) {
  previewPhoto.value = photo;
  previewVisible.value = true;
  previewLoading.value = true;
  previewSrc.value = "";
  try {
    const path = await loadAlbumPhoto(photo.id);
    previewSrc.value = convertFileSrc(path);
  } catch (reason) { photoError.value = `打开原图失败：${String(reason)}`; previewVisible.value = false; }
  finally { previewLoading.value = false; }
}

async function savePreview() {
  if (!previewPhoto.value || !previewSrc.value || savingPreview.value) return;
  savingPreview.value = true; photoError.value = "";
  try {
    const photo = previewPhoto.value;
    const extension = (photo.localPath || photo.url || "photo.jpg").split(".").pop()?.toLowerCase() || "jpg";
    const name = `${(photo.name || `qzone-${photo.picKey}`).replace(/[\\/:*?"<>|]/g, "_")}.${extension}`;
    const path = await save({ defaultPath: name, filters: [{ name: "图片", extensions: [extension, "jpg", "png", "webp"] }] });
    if (!path) return;
    if (photo.localPath) {
      await writeFile(path, new Uint8Array(await readFile(photo.localPath)));
    } else {
      const response = await window.fetch(previewSrc.value);
      await writeFile(path, new Uint8Array(await response.arrayBuffer()));
    }
  } catch (reason) { photoError.value = `保存照片失败：${String(reason)}`; }
  finally { savingPreview.value = false; }
}

function askDelete(album: AlbumSummary) { pendingDelete.value = album; confirmVisible.value = true; }

async function confirmDelete() {
  const album = pendingDelete.value;
  if (!album || deleting.value) return;
  deleting.value = true; error.value = "";
  try {
    await deleteArchivedAlbum(album.albumId);
    confirmVisible.value = false;
    await refreshAlbums();
  } catch (reason) { error.value = String(reason); }
  finally { deleting.value = false; }
}

async function retryFailed(album: AlbumSummary) {
  error.value = "";
  try {
    const reset = await resetFailedAlbumPhotos(album.albumId);
    if (reset > 0) await startBackup([album.albumId]);
    else error.value = "没有需要重试的失败照片";
  } catch (reason) { error.value = String(reason); }
}

onMounted(async () => {
  currentTime.value = Date.now();
  await refreshProgress();
  if (authStore.loggedIn) await refreshAlbums();
  if (running.value || rateLimited.value) beginPolling();
});
watch(() => authStore.loggedIn, async (loggedIn) => {
  if (loggedIn) {
    await refreshProgress();
    await refreshAlbums();
    if (running.value || rateLimited.value) beginPolling();
  } else {
    albums.value = [];
    photoDialogVisible.value = false;
  }
});
onBeforeUnmount(() => {
  if (timer) window.clearInterval(timer);
  imageObserver?.disconnect();
  Object.values(coverSources).forEach((url) => { if (url.startsWith("blob:")) URL.revokeObjectURL(url); });
  Object.values(photoSources).forEach((url) => { if (url.startsWith("blob:")) URL.revokeObjectURL(url); });
});
</script>

<template>
  <div class="albums-page">
    <section class="albums-hero surface-card">
      <div class="albums-hero-copy">
        <span class="albums-hero-icon"><i class="pi pi-images" /></span>
        <div><p class="section-kicker">ALBUM BACKUP</p><h2>相册备份</h2><p>把 QQ 空间相册的照片原图批量保存到本地，支持断点续传。</p></div>
      </div>
      <div class="albums-actions">
        <Button icon="pi pi-refresh" label="刷新列表" severity="secondary" text :loading="loading" :disabled="!authStore.loggedIn" @click="refreshAlbums" />
        <Button icon="pi pi-cloud-download" label="备份全部相册" :loading="starting" :disabled="running || rateWaiting || !authStore.loggedIn" @click="startBackup()" />
      </div>
    </section>

    <section v-if="running || progress.status !== 'idle'" class="albums-progress surface-card">
      <div class="albums-progress-head">
        <div class="section-heading"><div><p class="section-kicker">BACKUP JOB</p><h3>相册备份任务</h3></div><Tag :value="statusText" :severity="statusSeverity" /></div>
        <Button v-if="running" label="停止" icon="pi pi-stop" severity="warn" outlined size="small" @click="stopBackup" />
      </div>
      <p class="albums-progress-message">{{ progress.message }}</p>
      <ProgressBar v-if="running" class="albums-progress-bar" :value="progressPercent" :show-value="false" style="height: 7px" />
      <div class="albums-progress-meta">
        <span>相册 {{ progress.albumsDone }}/{{ progress.albumsTotal }}</span>
        <span>照片 {{ progress.photosDone }}/{{ progress.photosTotal }}</span>
        <span v-if="progress.photosFailed" class="albums-progress-failed">失败 {{ progress.photosFailed }}</span>
        <span v-if="rateLimited"><i class="pi pi-shield" /> 频率保护中，{{ rateWaiting ? `等待 ${remainingText} 后可继续` : "现在可以继续" }}</span>
      </div>
      <Button v-if="rateLimited && !rateWaiting" label="继续备份" icon="pi pi-play" size="small" @click="startBackup()" />
    </section>

    <p v-if="offlineNotice" class="archive-error is-warning"><i class="pi pi-info-circle" />{{ offlineNotice }}</p>
    <p v-if="error" class="archive-error"><i class="pi pi-exclamation-circle" />{{ error }}</p>

    <section v-if="albums.length" class="albums-grid" aria-label="相册列表">
      <article v-for="album in albums" :key="album.albumId" class="surface-card album-card">
        <div class="album-cover" :data-album-cover="album.albumId">
          <img v-if="coverSources[album.albumId]" :src="coverSources[album.albumId]" :alt="album.name" loading="lazy" />
          <i v-else class="pi pi-image" />
          <span class="album-count-badge"><i class="pi pi-camera" />{{ album.photoTotal }}</span>
        </div>
        <div class="album-card-body">
          <header><strong :title="album.name">{{ album.name }}</strong><Tag :value="albumStatusText(album).label" :severity="albumStatusText(album).severity" /></header>
          <p v-if="album.description" class="album-desc">{{ album.description }}</p>
          <small class="album-meta">远端 {{ album.photoTotal }} 张 · 本地 {{ album.backedCount }} 张<template v-if="album.modifyTime"> · 更新于 {{ formatTime(album.modifyTime) }}</template></small>
          <div class="album-actions">
            <Button :label="album.backedCount > 0 ? '继续备份' : '备份'" icon="pi pi-download" size="small" :loading="starting" :disabled="running || rateWaiting" @click="startBackup([album.albumId])" />
            <Button v-if="album.backedCount > 0 || album.failedCount > 0" label="照片" icon="pi pi-eye" size="small" severity="secondary" outlined @click="openPhotos(album)" />
            <Button v-if="album.failedCount > 0" :label="`重试失败 ${album.failedCount}`" icon="pi pi-replay" size="small" severity="warn" text :disabled="running || starting" @click="retryFailed(album)" />
            <Button v-if="album.backedCount > 0" label="删除备份" icon="pi pi-trash" size="small" severity="danger" text @click="askDelete(album)" />
          </div>
        </div>
      </article>
    </section>
    <section v-else-if="!loading" class="surface-card empty-state page-empty">
      <span><i class="pi pi-images" /></span>
      <h2>{{ authStore.loggedIn ? "暂无相册" : "请先登录 QQ 空间" }}</h2>
      <p>{{ authStore.loggedIn ? "刷新列表获取当前账号的相册，或先完成动态归档。" : "登录后即可查看并备份相册。" }}</p>
    </section>
    <p v-if="hasBackedUp" class="albums-footnote">已备份的照片保存在应用数据目录 images/albums 下，可在“归档内容”页导出为离线包。</p>

    <Dialog v-model:visible="photoDialogVisible" modal :draggable="false" maximizable class="albums-photo-dialog" :header="`相册照片 · ${photoAlbum?.name || ''}`">
      <p v-if="photoError" class="archive-error"><i class="pi pi-exclamation-circle" />{{ photoError }}</p>
      <div v-if="photos.length" class="albums-photo-grid">
        <button v-for="photo in photos" :key="photo.id" type="button" class="albums-photo-tile" :title="photo.name || photo.picKey" @click="openPreview(photo)">
          <img v-if="photoSources[photo.id]" :src="photoSources[photo.id]" loading="lazy" :alt="photo.name || '相册照片'" @error="retryPhotoImage(photo.id)" />
          <span v-else class="albums-photo-placeholder">
            <i :class="photoErrors[photo.id] ? 'pi pi-exclamation-triangle' : 'pi pi-spin pi-spinner'" />
            <small v-if="photoErrors[photo.id]" role="button" @click.stop="retryPhotoImage(photo.id)">重试</small>
            <small v-if="photo.status === 'failed'" class="albums-photo-failed">未备份</small>
          </span>
        </button>
      </div>
      <p v-else-if="!photoLoading" class="albums-photo-empty">该相册还没有已备份的照片。</p>
      <div class="albums-photo-more">
        <i v-if="photoLoading" class="pi pi-spin pi-spinner" />
        <Button v-else-if="!allPhotosLoaded" label="加载更多" severity="secondary" text @click="loadPhotos()" />
        <span v-else-if="photos.length">已经到底了 · 共 {{ photoTotal }} 张</span>
      </div>
    </Dialog>

    <Dialog v-model:visible="previewVisible" modal :draggable="false" class="albums-preview-dialog" :header="previewPhoto?.name || '照片原图'">
      <div class="albums-preview-stage">
        <i v-if="previewLoading" class="pi pi-spin pi-spinner" />
        <img v-else-if="previewSrc" :src="previewSrc" :alt="previewPhoto?.name || '照片原图'" />
      </div>
      <template #footer>
        <Button label="关闭" severity="secondary" text @click="previewVisible = false" />
        <Button label="保存照片" icon="pi pi-download" :loading="savingPreview" @click="savePreview" />
      </template>
    </Dialog>

    <Dialog v-model:visible="confirmVisible" modal :closable="!deleting" :draggable="false" class="delete-dialog" header="删除本地相册备份？">
      <div class="delete-dialog-content">
        <span class="delete-warning"><i class="pi pi-trash" /></span>
        <div><p>将删除「{{ pendingDelete?.name }}」在本地备份的 {{ pendingDelete?.backedCount }} 张照片与记录。</p><small>QQ 空间云端照片不受影响，此操作无法撤销。</small></div>
      </div>
      <template #footer>
        <Button label="取消" severity="secondary" text :disabled="deleting" @click="confirmVisible = false" />
        <Button label="确认删除" icon="pi pi-trash" severity="danger" :loading="deleting" @click="confirmDelete" />
      </template>
    </Dialog>
  </div>
</template>
