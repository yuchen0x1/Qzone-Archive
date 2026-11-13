<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { withBase } from "vitepress";
import packageJson from "../../../../package.json";
type Platform = "windows" | "macos" | "android" | "ios" | "linux";
type ThemeMode = "system" | "light" | "dark";
type ReleaseAsset = { name: string; size?: number; browser_download_url: string };
type LatestRelease = { tag_name: string; html_url: string; published_at?: string; created_at?: string; assets: ReleaseAsset[] };

const repoUrl = "https://github.com/yuchen0x1/Qzone-Archive";
const trustedReleaseHosts = new Set(["github.com", "api.github.com", "gitee.com", "gitcode.com"]);
function trustedSourceUrl(raw: string | undefined): string | undefined {
  if (!raw?.trim()) return undefined;
  try {
    const url = new URL(raw.trim());
    if (url.protocol !== "https:" || url.port || url.username || url.password || !trustedReleaseHosts.has(url.hostname.toLowerCase())) return undefined;
    return url.toString();
  } catch {
    return undefined;
  }
}
const configuredReleaseApiUrl = trustedSourceUrl(import.meta.env.VITE_UPDATE_API_URL);
const configuredReleaseUrl = trustedSourceUrl(import.meta.env.VITE_RELEASES_URL);
const releaseApiUrl = configuredReleaseApiUrl || "https://api.github.com/repos/yuchen0x1/Qzone-Archive/releases/latest";
const releaseUrl = configuredReleaseUrl
  ? configuredReleaseUrl.replace(/\/$/, "") + "/latest"
  : repoUrl + "/releases";
const currentVersion = String(packageJson.version);
const scrolled = ref(false);
const selectedPlatform = ref<Platform>("windows");
const themeMode = ref<ThemeMode>("system");
const revealObserver = ref<IntersectionObserver | null>(null);
const release = ref<LatestRelease | null>(null);
const releaseLoading = ref(false);
const releaseError = ref("");

const platforms = [
  { id: "windows", label: "Windows", format: ".exe", arch: "x64", version: "Windows 10+", note: "安装后即可开始归档。" },
  { id: "macos", label: "macOS", format: ".dmg", arch: "Apple / Intel", version: "macOS 11+", note: "请按芯片架构选择安装包。" },
  { id: "android", label: "Android", format: ".apk", arch: "arm64", version: "Android 8+", note: "适合在移动设备上查看和管理归档。" },
  { id: "ios", label: "iOS", format: ".ipa", arch: "arm64", version: "iOS 15+", note: "未签名 IPA，需使用侧载工具安装。" },
  { id: "linux", label: "Linux", format: ".AppImage", arch: "x86_64", version: "主流发行版", note: "赋予执行权限后即可运行。" },
] as const;

const themeModes: { id: ThemeMode; label: string }[] = [
  { id: "system", label: "跟随系统" },
  { id: "light", label: "浅色" },
  { id: "dark", label: "深色" },
];

const activePlatform = computed(() => platforms.find((item) => item.id === selectedPlatform.value) ?? platforms[0]);
const activeAsset = computed(() => release.value ? findAsset(selectedPlatform.value, release.value.assets) : undefined);
const releasePageUrl = computed(() => trustedSourceUrl(release.value?.html_url) ?? releaseUrl);
const downloadUrl = computed(() => activeAsset.value?.browser_download_url ?? releasePageUrl.value);
const releaseVersion = computed(() => release.value?.tag_name.replace(/^v/i, "") ?? "");
const releaseIsNewer = computed(() => release.value ? compareVersions(releaseVersion.value, currentVersion) > 0 : false);
const releasePublishedLabel = computed(() => {
  const raw = release.value?.published_at || release.value?.created_at;
  if (!raw) return "";
  const date = new Date(raw);
  return Number.isNaN(date.getTime()) ? "" : ` · 发布于 ${date.toLocaleDateString()}`;
});
const assetSizeLabel = computed(() => {
  if (!activeAsset.value?.size) return "";
  return ` · ${(activeAsset.value.size / 1024 / 1024).toFixed(1)} MB`;
});

function compareVersions(left: string, right: string): number {
  const parse = (value: string) => value.replace(/^v/i, "").split("-")[0].split(".").map((part) => Number.parseInt(part, 10) || 0);
  const a = parse(left);
  const b = parse(right);
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference) return difference;
  }
  return 0;
}

function findAsset(platform: Platform, assets: ReleaseAsset[]): ReleaseAsset | undefined {
  const agent = navigator.userAgent.toLowerCase();
  const isAppleSilicon = agent.includes("arm64") || agent.includes("aarch64") || navigator.platform === "MacARM";
  const matches: Record<Platform, (asset: ReleaseAsset) => boolean> = {
    windows: (asset) => asset.name.endsWith("x64-setup.exe"),
    macos: (asset) => asset.name.endsWith(isAppleSilicon ? "aarch64.dmg" : "x64.dmg"),
    android: (asset) => asset.name.toLowerCase().includes("android") && asset.name.endsWith(".apk"),
    ios: (asset) => asset.name.endsWith(".ipa"),
    linux: (asset) => asset.name.endsWith(".AppImage"),
  };
  return assets.find((asset) => trustedSourceUrl(asset.browser_download_url) && matches[platform](asset));
}

async function checkLatestRelease() {
  if (releaseLoading.value) return;
  releaseLoading.value = true;
  releaseError.value = "";
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), 12000);
  try {
    const response = await fetch(releaseApiUrl, {
      headers: { Accept: "application/json" },
      cache: "no-store",
      signal: controller.signal,
    });
    if (!response.ok) throw new Error(`发布仓库返回 HTTP ${response.status}`);
    release.value = await response.json() as LatestRelease;
  } catch (error) {
    releaseError.value = error instanceof Error && error.name === "AbortError" ? "检查超时，请直接打开 Release 页面。" : "暂时无法读取最新版本，请直接打开 Release 页面。";
  } finally {
    window.clearTimeout(timeout);
    releaseLoading.value = false;
  }
}

function updateScrollState() { scrolled.value = window.scrollY > 8; }
function setDocumentTheme(mode: ThemeMode) {
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.classList.toggle("dark", mode === "dark" || (mode === "system" && prefersDark));
}
function onSystemThemeChange() { if (themeMode.value === "system") setDocumentTheme("system"); }
function setupRevealObservers() {
  revealObserver.value = new IntersectionObserver((entries) => {
    entries.forEach((entry) => {
      if (entry.isIntersecting) {
        entry.target.classList.add("is-visible");
        revealObserver.value?.unobserve(entry.target);
      }
    });
  }, { threshold: 0.12, rootMargin: "0px 0px -8% 0px" });
  document.querySelectorAll(".js-reveal").forEach((el) => revealObserver.value?.observe(el));
}

watch(themeMode, (mode) => { localStorage.setItem("qzonearchive-theme", mode); setDocumentTheme(mode); });

onMounted(() => {
  const storedTheme = localStorage.getItem("qzonearchive-theme");
  if (storedTheme === "light" || storedTheme === "dark" || storedTheme === "system") themeMode.value = storedTheme;
  setDocumentTheme(themeMode.value);
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", onSystemThemeChange);
  const agent = navigator.userAgent.toLowerCase();
  if (agent.includes("android")) selectedPlatform.value = "android";
  else if (agent.includes("iphone") || agent.includes("ipad") || agent.includes("ipod")) selectedPlatform.value = "ios";
  else if (agent.includes("mac")) selectedPlatform.value = "macos";
  else if (agent.includes("linux")) selectedPlatform.value = "linux";
  updateScrollState();
  window.addEventListener("scroll", updateScrollState, { passive: true });
  setupRevealObservers();
  void checkLatestRelease();
});

onBeforeUnmount(() => {
  window.removeEventListener("scroll", updateScrollState);
  window.matchMedia("(prefers-color-scheme: dark)").removeEventListener("change", onSystemThemeChange);
  revealObserver.value?.disconnect();
});
</script>

<template>
  <div class="archive-home">
    <header class="archive-nav" :class="{ 'is-scrolled': scrolled }">
      <div class="archive-shell archive-nav__inner">
        <a class="archive-brand" :href="withBase('/')" aria-label="QQ空间归档首页">
          <img :src="withBase('/qzone-archive-icon.png')" alt="" />
          <span>QQ空间归档</span>
        </a>
        <nav class="archive-nav__links" aria-label="首页导航">
          <a href="#features">能做什么</a>
          <a href="#screens">界面</a>
          <a href="#download">下载</a>
          <a :href="withBase('/intro/')">文档</a>
        </nav>
        <div class="archive-theme" role="group" aria-label="颜色模式切换">
          <button v-for="mode in themeModes" :key="mode.id" type="button" :class="{ active: themeMode === mode.id }" :title="mode.label" :aria-label="mode.label" @click="themeMode = mode.id">
            <span v-if="mode.id === 'system'">▣</span><span v-else-if="mode.id === 'light'">☼</span><span v-else>◐</span>
          </button>
        </div>
        <a class="archive-button archive-button--small" href="#download">下载</a>
      </div>
    </header>

    <main>
      <section class="archive-hero">
        <div class="archive-shell archive-hero__grid">
          <div class="archive-hero__copy js-reveal">
            <p class="archive-kicker">给自己的空间，留一份副本</p>
            <h1>把 QQ 空间记忆，留在自己的设备上。</h1>
            <p class="archive-lead">空间归档将当前账号可访问的动态、照片、视频和互动记录整理为本地档案。可搜索、可继续、可离线导出。</p>
            <div class="archive-actions">
              <a class="archive-button" href="#download">下载 {{ activePlatform.label }} 版 <span>↗</span></a>
              <a class="archive-button archive-button--quiet" :href="withBase('/intro/')">先看看使用文档</a>
            </div>
            <p class="archive-boundary">本地运行 · SQLite 存储 · 不提供云端账号系统</p>
            <p class="archive-status"><span aria-hidden="true"></span>已停止维护 · 2026-11-13 存档 · 最后版本 v1.3.5</p>
          </div>
          <div class="archive-hero__preview js-reveal" style="transition-delay: 100ms">
            <div class="archive-photo-frame"><img :src="withBase('/screens/dashboard.png')" alt="QQ空间归档总览界面" /></div>
            <div class="archive-caption"><span>总览</span><span>本地归档统计与最近动态</span></div>
          </div>
        </div>
      </section>

      <section id="features" class="archive-section archive-features">
        <div class="archive-shell">
          <div class="archive-section__intro js-reveal"><p class="archive-kicker">它能做什么</p><h2>不是把内容搬走，<br />而是把关系保存下来。</h2><p>从动态正文到评论回复，从媒体到归档进度，重要的上下文都留在一个可以继续使用的本地档案里。</p></div>
          <div class="archive-feature-list">
            <article class="archive-feature js-reveal"><span class="archive-feature__number">01</span><div><h3>查得到</h3><p>按内容、昵称或 QQ 号搜索动态，按本人动态、其他动态和留言分类浏览。</p></div><strong>本地检索</strong></article>
            <article class="archive-feature js-reveal" style="transition-delay: 70ms"><span class="archive-feature__number">02</span><div><h3>接得上</h3><p>网络中断或触发频率保护后，可以按上次位置继续处理。</p></div><strong>断点续传</strong></article>
            <article class="archive-feature js-reveal" style="transition-delay: 140ms"><span class="archive-feature__number">03</span><div><h3>带得走</h3><p>将全部或选中的归档导出为独立 HTML 文件，不打开应用也能在浏览器里查看。</p></div><strong>HTML 导出</strong></article>
            <article class="archive-feature js-reveal" style="transition-delay: 210ms"><span class="archive-feature__number">04</span><div><h3>由你掌握</h3><p>多账号数据隔离保存在设备本地，媒体缓存、导出文件和删除操作都由你控制。</p></div><strong>本地优先</strong></article>
          </div>
        </div>
      </section>

      <section id="screens" class="archive-section archive-screens">
        <div class="archive-shell">
          <div class="archive-section__bar js-reveal"><div><p class="archive-kicker">在应用里</p><h2>从任务进度，到一眼看懂的档案。</h2></div><a :href="withBase('/first-archive/')">查看首次归档指南 →</a></div>
          <div class="archive-screen-grid">
            <figure class="archive-screen archive-screen--large js-reveal"><img :src="withBase('/screens/archive.png')" alt="归档内容浏览界面" /><figcaption><b>归档内容</b><span>搜索、分类、查看互动关系</span></figcaption></figure>
            <figure class="archive-screen js-reveal" style="transition-delay: 80ms"><img :src="withBase('/screens/media.png')" alt="媒体时光轴界面" /><figcaption><b>媒体时光轴</b><span>按年份浏览照片与视频</span></figcaption></figure>
            <figure class="archive-screen js-reveal" style="transition-delay: 160ms"><img :src="withBase('/screens/tasks.png')" alt="归档任务界面" /><figcaption><b>归档任务</b><span>进度、请求记录与异常重试</span></figcaption></figure>
          </div>
        </div>
      </section>

      <section id="how" class="archive-section archive-process">
        <div class="archive-shell"><div class="archive-section__intro js-reveal"><p class="archive-kicker">使用流程</p><h2>四步，得到一份自己的副本。</h2></div>
          <ol class="archive-timeline">
            <li class="js-reveal"><span>01</span><h3>登录</h3><p>使用二维码登录；桌面端也支持网页登录。</p></li>
            <li class="js-reveal" style="transition-delay: 70ms"><span>02</span><h3>开始归档</h3><p>在归档任务中启动读取，任务状态实时可见。</p></li>
            <li class="js-reveal" style="transition-delay: 140ms"><span>03</span><h3>继续任务</h3><p>遇到中断或限流，等待后从保存的位置继续。</p></li>
            <li class="js-reveal" style="transition-delay: 210ms"><span>04</span><h3>浏览与导出</h3><p>搜索本地记录，或生成可离线打开的 HTML。</p></li>
          </ol>
        </div>
      </section>

      <section id="data" class="archive-section archive-trust">
        <div class="archive-shell archive-trust__inner js-reveal"><div><p class="archive-kicker">LOCAL FIRST</p><h2>数据从哪里来，<br />最后留在哪里。</h2></div><div class="archive-trust__copy"><p>应用直接连接 QQ 相关接口读取当前账号有权访问的内容，在本机处理归档任务，将数据写入 SQLite，并保存媒体缓存。</p><p>登录凭证只由 Rust 后端在内存中处理。项目不提供自建云端存储；导出的文件由你自行保管。</p><a :href="withBase('/data-and-safety/')">阅读数据与安全说明 →</a></div></div>
      </section>

      <section id="download" class="archive-section archive-download">
        <div class="archive-shell"><div class="archive-section__intro js-reveal"><p class="archive-kicker">历史下载</p><h2>选择你的设备。</h2><p>从 GitHub Releases 获取最后发布的 Windows 版本。其他平台可选择历史版本或研究源码，项目不再更新。</p></div>
          <div class="archive-release-status js-reveal" aria-live="polite"><span v-if="releaseLoading">正在检查最新版本…</span><span v-else-if="releaseError" class="archive-release-status--error">{{ releaseError }}</span><span v-else-if="release">{{ releaseIsNewer ? `发现新版本 v${releaseVersion}` : `当前已是最新版本 v${currentVersion}` }}{{ releasePublishedLabel }}</span><span v-else>尚未检查版本。</span><button type="button" @click="checkLatestRelease">{{ releaseLoading ? "检查中…" : "重新检查" }}</button></div>
          <div class="archive-downloader js-reveal"><div class="archive-platform-tabs" role="tablist" aria-label="选择平台"><button v-for="platform in platforms" :key="platform.id" type="button" role="tab" :id="`tab-${platform.id}`" :aria-selected="selectedPlatform === platform.id" :aria-controls="`panel-${platform.id}`" :class="{ active: selectedPlatform === platform.id }" @click="selectedPlatform = platform.id">{{ platform.label }}</button></div><div :id="`panel-${selectedPlatform}`" class="archive-download__body" role="tabpanel" :aria-labelledby="`tab-${selectedPlatform}`"><div><h3>{{ activePlatform.label }}</h3><p>{{ activePlatform.note }}</p><div class="archive-specs"><span><small>格式</small>{{ activePlatform.format }}</span><span><small>架构</small>{{ activePlatform.arch }}</span><span><small>系统</small>{{ activePlatform.version }}</span></div><small v-if="activeAsset" class="archive-download__asset">{{ activeAsset.name }}{{ assetSizeLabel }}</small></div><a class="archive-button" :href="downloadUrl" target="_blank" rel="noreferrer">{{ activeAsset ? "下载最新安装包" : "打开 Release 页面" }} ↗</a></div></div>
          <p class="archive-download-note">归档范围受 QQ 空间接口和账号权限限制。<a :href="withBase('/data-and-safety/')">了解使用边界</a></p>
        </div>
      </section>
    </main>

    <footer class="archive-footer"><div class="archive-shell"><div class="archive-brand"><img :src="withBase('/qzone-archive-icon.png')" alt="" /><span>QQ空间归档</span></div><p>把个人记忆，留在自己的设备上。</p><nav><a :href="withBase('/intro/')">使用文档</a><a :href="repoUrl">GitHub 存档仓库</a><a :href="`${repoUrl}/blob/main/LICENSE`">许可证</a></nav></div></footer>
  </div>
</template>
