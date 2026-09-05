<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { storeToRefs } from "pinia";
import { useRoute, useRouter } from "vue-router";
import { arch, platform } from "@tauri-apps/plugin-os";
import Button from "primevue/button";
import Dialog from "primevue/dialog";
import Drawer from "primevue/drawer";
import Popover from "primevue/popover";
import LoginDialog from "../components/LoginDialog.vue";
import sidebarToggleIcon from "../assets/sidebar-toggle.png";
import { useAppStore } from "../stores/app";
import { useAuthStore } from "../stores/auth";
import { assetForPlatform, checkForUpdate, updateAssetLabel, type UpdateInfo, type UpdatePlatform } from "../utils/updater";

defineProps<{ pageTitle: string }>();
const appStore = useAppStore();
const authStore = useAuthStore();
const router = useRouter();
const route = useRoute();
const accountPopover = ref<InstanceType<typeof Popover>>();
const moreVisible = ref(false);
const logoutLoading = ref(false);
const { darkMode, sidebarCollapsed, themeIcon } = storeToRefs(appStore);
const { loggedIn, user } = storeToRefs(authStore);
const showQzoneButton = computed(() => {
  const currentPlatform = platform();
  return currentPlatform !== "android" && currentPlatform !== "ios";
});
const navigation = [
  { label: "概览", icon: "pi pi-home", to: "/" },
  { label: "归档", icon: "pi pi-inbox", to: "/archives" },
  { label: "联系人", icon: "pi pi-users", to: "/contacts" },
  { label: "媒体", icon: "pi pi-images", to: "/media" },
  { label: "任务", icon: "pi pi-sync", to: "/tasks" },
  { label: "回收站", icon: "pi pi-trash", to: "/recycle-bin" },
];
const settingsItem = { label: "设置", icon: "pi pi-cog", to: "/settings" };
const mobileNavigation = [navigation[0], navigation[1], navigation[3], navigation[4]];
const mobileMoreNavigation = [navigation[2], settingsItem];
const moreActive = computed(() => mobileMoreNavigation.some((item) => item.to === route.path));
function qzoneUrl() {
  const uin = user.value?.uin;
  if (platform() === "android") return uin ? `https://m.qzone.qq.com/${uin}` : "https://m.qzone.qq.com";
  return uin ? `https://user.qzone.qq.com/${uin}` : "https://user.qzone.qq.com";
}
async function openQzoneWindow() {
  try {
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const existing = await WebviewWindow.getByLabel("qzone-browser");
    if (existing) { await existing.setFocus(); return; }
    await new WebviewWindow("qzone-browser", {
      url: qzoneUrl(), title: "QQ 空间", width: 1000, height: 720, minWidth: 480, minHeight: 500, center: true,
    });
  } catch {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(qzoneUrl());
  }
}

function handleAccountClick(event: MouseEvent) {
  if (loggedIn.value) accountPopover.value?.toggle(event);
  else authStore.openLogin();
}

const startupUpdate = ref<UpdateInfo | null>(null);
const startupUpdateVisible = ref(false);

onMounted(async () => {
  // 启动时静默检查一次更新，仅在发现新版本时弹窗提示（失败不打扰用户）。
  try {
    startupUpdate.value = await checkForUpdate();
    if (startupUpdate.value) startupUpdateVisible.value = true;
  } catch { /* 静默失败 */ }
});
function currentUpdateAsset() {
  if (!startupUpdate.value) return undefined;
  const currentPlatform = platform() as UpdatePlatform;
  return assetForPlatform(startupUpdate.value.assets, currentPlatform, arch());
}
async function openUpdatePage() {
  if (!startupUpdate.value) return;
  startupUpdateVisible.value = false;
  try {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(currentUpdateAsset()?.browser_download_url ?? startupUpdate.value.release_url);
  } catch { /* 打开失败时保留窗口，用户可重试 */ }
}
async function logout() {
  if (logoutLoading.value) return;
  logoutLoading.value = true;
  try {
    accountPopover.value?.hide();
    await authStore.logout();
    await router.push("/");
  } finally { logoutLoading.value = false; }
}
</script>

<template>
  <div class="app-shell" :class="{ 'app-dark': darkMode, 'sidebar-collapsed': sidebarCollapsed }">
    <aside class="desktop-sidebar">
      <div class="brand">
        <button
          class="brand-mark"
          type="button"
          :class="{ 'is-toggle': sidebarCollapsed }"
          :title="sidebarCollapsed ? '展开侧边栏' : undefined"
          aria-label="折叠侧边栏"
          @click="sidebarCollapsed && appStore.toggleSidebar()"
        >
          <i class="pi pi-box brand-app-icon" />
          <img v-if="sidebarCollapsed" class="brand-toggle-icon" :src="sidebarToggleIcon" alt="" />
        </button>
        <div class="brand-copy"><strong>空间归档</strong><span>Qzone Archive</span></div>
      </div>
      <button v-if="!sidebarCollapsed" class="sidebar-collapse-btn" type="button" title="折叠侧边栏" aria-label="折叠侧边栏" @click="appStore.toggleSidebar">
        <img class="sidebar-collapse-icon" :src="sidebarToggleIcon" alt="" />
      </button>
      <nav class="side-navigation" aria-label="主要导航">
        <RouterLink v-for="item in navigation" :key="item.to" :to="item.to" :title="sidebarCollapsed ? item.label : undefined">
          <i :class="item.icon" /><span>{{ item.label }}</span>
        </RouterLink>
      </nav>
      <div class="sidebar-footer">
        <RouterLink :to="settingsItem.to" :title="sidebarCollapsed ? settingsItem.label : undefined" class="sidebar-settings-link">
          <i :class="settingsItem.icon" /><span>{{ settingsItem.label }}</span>
        </RouterLink>
      </div>
    </aside>
    <div class="app-workspace">
      <header class="topbar">
        <div><p class="topbar-eyebrow">QZONE ARCHIVE</p><h1>{{ pageTitle }}</h1></div>
        <div class="topbar-actions">
          <Button :icon="themeIcon" severity="secondary" text rounded aria-label="切换主题" @click="appStore.toggleTheme" />
          <button class="account-chip" type="button" :aria-label="loggedIn ? '打开账号菜单' : '登录 QQ 空间'" @click="handleAccountClick">
            <span class="account-avatar">
              <img v-if="user?.avatarImage" :src="user.avatarImage" alt="" />
              <i v-else class="pi pi-user" />
            </span>
            <span class="account-copy"><strong>{{ user?.nickname ?? '尚未登录' }}</strong><small>{{ user ? `QQ ${user.uin}` : '登录 QQ 空间' }}</small></span>
            <span v-if="loggedIn" class="account-menu-indicator"><i class="pi pi-angle-down account-menu-arrow" /><i class="pi pi-sign-out account-menu-logout" /></span>
          </button>
          <Popover ref="accountPopover" class="account-popover">
            <div class="account-popover-profile"><span class="account-popover-avatar"><img v-if="user?.avatarImage" :src="user.avatarImage" alt="" /><i v-else class="pi pi-user" /></span><div><strong>{{ user?.nickname }}</strong><span>QQ {{ user?.uin }}</span></div></div>
            <div class="account-popover-divider" />
            <Button v-if="showQzoneButton" label="QQ 空间" icon="pi pi-globe" severity="secondary" text @click="openQzoneWindow(); accountPopover?.hide()" />
            <Button label="退出登录" icon="pi pi-sign-out" severity="danger" text :loading="logoutLoading" @click="logout" />
          </Popover>
        </div>
      </header>
      <main class="page-content"><slot /></main>
    </div>
    <nav class="mobile-navigation" aria-label="移动端导航">
      <RouterLink v-for="item in mobileNavigation" :key="item.to" :to="item.to">
        <i :class="item.icon" /><span>{{ item.label }}</span>
      </RouterLink>
      <button class="mobile-navigation-item" :class="{ 'is-active': moreActive }" type="button" aria-label="打开更多页面" @click="moreVisible = true">
        <i class="pi pi-ellipsis-h" /><span>更多</span>
      </button>
    </nav>
    <Drawer v-model:visible="moreVisible" position="bottom" header="更多" class="mobile-more-drawer">
      <nav class="mobile-more-navigation" aria-label="更多页面">
        <RouterLink v-for="item in mobileMoreNavigation" :key="item.to" :to="item.to" @click="moreVisible = false">
          <span class="mobile-more-icon"><i :class="item.icon" /></span>
          <span><strong>{{ item.label }}</strong><small>{{ item.to === '/contacts' ? '查看与你互动过的联系人' : '归档频率、主题与数据管理' }}</small></span>
          <i class="pi pi-angle-right" />
        </RouterLink>
      </nav>
    </Drawer>
    <LoginDialog />
    <Dialog v-model:visible="startupUpdateVisible" modal :draggable="false" class="update-dialog" header="发现新版本">
      <div v-if="startupUpdate" class="update-dialog-content">
        <p>新版本 <strong>v{{ startupUpdate.latest_version }}</strong> 已发布（当前 v{{ startupUpdate.current_version }}），建议尽快升级以获得修复与新功能。</p>
        <ul v-if="startupUpdate.assets.length" class="update-assets">
          <li v-for="asset in startupUpdate.assets" :key="asset.name">
            <span>{{ updateAssetLabel(asset.name) }}（{{ asset.name }}）</span>
            <small>{{ (asset.size / 1024 / 1024).toFixed(1) }} MB</small>
          </li>
        </ul>
      </div>
      <template #footer>
        <Button label="暂不更新" severity="secondary" text @click="startupUpdateVisible = false" />
        <Button label="下载对应安装包" icon="pi pi-download" @click="openUpdatePage" />
      </template>
    </Dialog>
  </div>
</template>
