<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { storeToRefs } from "pinia";
import Button from "primevue/button";
import ProgressBar from "primevue/progressbar";
import Tag from "primevue/tag";
import {
  cancelFeedArchive, clearResolvedArchiveSkips, getArchiveProgress, listArchiveSkips, retryAllArchiveSkips, retryArchiveSkip, startFeedArchive,
  type ArchiveProgress, type ArchiveSkipItem,
} from "../utils/qzone";
import { useAuthStore } from "../stores/auth";
import { getArchiveInterval } from "../utils/appSettings";

const authStore = useAuthStore();
const { loggedIn } = storeToRefs(authStore);
const progress = ref<ArchiveProgress>({ status: "idle", pages: 0, fetched: 0, saved: 0, skipped: 0, message: "尚未开始归档" });
const skips = ref<ArchiveSkipItem[]>([]);
const retryingId = ref<number>();
const skipNotice = ref("");
const skipFilter = ref<"all" | "pending" | "resolved">("all");
const clearingResolved = ref(false);
const batchRetrying = ref(false);
const batchStopping = ref(false);
const currentTime = ref(Date.now());
let timer: number | undefined;
const running = computed(() => progress.value.status === "running");
const rateLimited = computed(() => progress.value.status === "limited");
const remainingSeconds = computed(() => Math.max(0, Math.ceil((Number(progress.value.retryAt || 0) * 1000 - currentTime.value) / 1000)));
const rateWaiting = computed(() => rateLimited.value && remainingSeconds.value > 0);
const remainingText = computed(() => `${String(Math.floor(remainingSeconds.value / 60)).padStart(2, "0")}:${String(remainingSeconds.value % 60).padStart(2, "0")}`);
const severity = computed(() => ({ completed: "success", error: "danger", cancelled: "warn", limited: "warn", running: "info", idle: "secondary" }[progress.value.status]));
const statusText = computed(() => ({ idle: "未开始", running: "进行中", completed: "已完成", cancelled: "已取消", limited: "频率保护", error: "失败" }[progress.value.status]));
const pendingSkips = computed(() => skips.value.filter((item) => !item.resolvedAt));
const resolvedSkips = computed(() => skips.value.filter((item) => item.resolvedAt));
const batchProgress = computed(() => progress.value.batchRetry);
const batchProgressText = computed(() => {
  const b = batchProgress.value;
  if (!b) return "";
  return `正在重试 ${Math.min(b.current, b.total)}/${b.total} · 已恢复 ${b.recovered}${b.failed ? ` · 失败 ${b.failed}` : ""}`;
});
const filteredSkips = computed(() => {
  if (skipFilter.value === "pending") return pendingSkips.value;
  if (skipFilter.value === "resolved") return resolvedSkips.value;
  return skips.value;
});
const filterOptions = [
  { label: "全部", value: "all" as const },
  { label: "待重试", value: "pending" as const },
  { label: "已恢复", value: "resolved" as const },
];
const filterCount = (value: (typeof filterOptions)[number]["value"]) =>
  value === "all" ? skips.value.length : value === "pending" ? pendingSkips.value.length : resolvedSkips.value.length;

async function refresh() {
  try { progress.value = await getArchiveProgress(); } catch { /* 保留当前状态 */ }
  if (progress.value.batchRetry) batchRetrying.value = true;
  if (!loggedIn.value) { skips.value = []; return; }
  try { skips.value = await listArchiveSkips(); } catch { /* 保留当前列表 */ }
}
function beginPolling() { window.clearInterval(timer); timer = window.setInterval(() => { currentTime.value = Date.now(); void refresh(); }, 600); }
async function start() {
  if (!loggedIn.value) return;
  beginPolling();
  try { progress.value = await startFeedArchive(getArchiveInterval()); }
  catch { await refresh(); }
  finally { await refresh(); if (progress.value.status === "limited") beginPolling(); else { window.clearInterval(timer); timer = undefined; } }
}
async function cancel() { await cancelFeedArchive(); await refresh(); }
async function retrySkip(item: ArchiveSkipItem) {
  retryingId.value = item.id;
  skipNotice.value = "";
  try {
    const result = await retryArchiveSkip(item.id);
    skipNotice.value = result.message;
  } catch (error) {
    skipNotice.value = String(error);
  } finally {
    retryingId.value = undefined;
    await refresh();
  }
}
async function clearResolved() {
  clearingResolved.value = true;
  skipNotice.value = "";
  try {
    const removed = await clearResolvedArchiveSkips();
    skipNotice.value = `已清理 ${removed} 条已恢复记录`;
    if (skipFilter.value === "resolved") skipFilter.value = "all";
  } catch (error) {
    skipNotice.value = String(error);
  } finally {
    clearingResolved.value = false;
    await refresh();
  }
}
async function retryAllPending() {
  batchRetrying.value = true;
  skipNotice.value = "";
  beginPolling();
  try {
    const result = await retryAllArchiveSkips(getArchiveInterval());
    skipNotice.value = batchStopping.value
      ? `已停止批量重试：本次恢复 ${result.recovered} 条${result.failed ? `，失败 ${result.failed} 条` : ""}`
      : result.total === 0
        ? "没有待重试的异常记录"
        : `批量重试完成：共 ${result.total} 条，成功恢复 ${result.recovered} 条${result.failed ? `，失败 ${result.failed} 条` : ""}${result.recoveredRecords ? `，找回 ${result.recoveredRecords} 条接口记录` : ""}`;
  } catch (error) {
    skipNotice.value = String(error);
  } finally {
    await refresh();
    if (!progress.value.batchRetry) {
      batchRetrying.value = false;
      batchStopping.value = false;
    }
    window.clearInterval(timer);
    timer = undefined;
  }
}
async function stopBatchRetry() {
  if (batchStopping.value) return;
  batchStopping.value = true;
  await cancelFeedArchive();
}
function formatTime(timestamp?: number) {
  return timestamp ? new Date(timestamp * 1000).toLocaleString("zh-CN", { hour12: false }) : "—";
}
function offsetLabel(item: ArchiveSkipItem) {
  if (item.offsetAdvance <= 0) return `${item.cursorOffset}（待定位）`;
  const end = item.cursorOffset + item.offsetAdvance - 1;
  return end > item.cursorOffset ? `${item.cursorOffset}–${end}` : String(item.cursorOffset);
}
onMounted(async () => { await refresh(); currentTime.value = Date.now(); if (running.value || rateLimited.value || batchRetrying.value || batchProgress.value) beginPolling(); });
onBeforeUnmount(() => window.clearInterval(timer));
</script>

<template>
  <section class="surface-card task-card">
    <div class="section-heading"><div><p class="section-kicker">ARCHIVE JOB</p><h3>QQ 空间动态归档</h3></div><Tag :value="statusText" :severity="severity" /></div>
    <p class="task-message">{{ progress.message }}</p>
    <ProgressBar v-if="running" mode="indeterminate" style="height: 7px" />
    <div v-if="rateLimited" class="task-rate-limit"><span><i class="pi pi-shield" /></span><div><strong>接口频率保护</strong><p>为防止接口请求过于频繁，每 10 分钟最多请求 300 页。归档进度已保存，{{ rateWaiting ? `等待 ${remainingText} 后可继续` : "现在可以继续归档" }}。</p></div><b v-if="rateWaiting">{{ remainingText }}</b></div>
    <div v-if="batchRetrying && batchProgress" class="task-batch-progress"><span><i class="pi pi-spin pi-spinner" /></span><div><strong>{{ batchStopping ? "正在停止批量重试…" : "批量重试异常位置" }}</strong><p>{{ batchProgressText }}{{ batchStopping ? " · 等待当前请求结束后停止" : "" }}</p><ProgressBar :value="(Math.min(batchProgress.current, batchProgress.total) / batchProgress.total) * 100" :show-value="false" style="height: 6px" /></div></div>
    <div class="task-stats"><div><span>已读取页数</span><strong>{{ progress.pages }}</strong></div><div><span>接口记录</span><strong>{{ progress.fetched }}</strong></div><div><span>写入记录</span><strong>{{ progress.saved }}</strong></div><div><span>待重试异常</span><strong>{{ progress.skipped }}</strong></div></div>
    <div v-if="!loggedIn" class="task-login-notice"><span><i class="pi pi-lock" /></span><div><strong>请先登录 QQ 空间</strong><p>登录后才能创建或继续归档任务。</p></div><Button label="立即登录" icon="pi pi-sign-in" size="small" @click="authStore.openLogin" /></div>
    <div class="task-actions">
      <Button :label="running ? '归档中…' : batchRetrying ? '批量重试中…' : rateWaiting ? `请等待 ${remainingText}` : rateLimited ? '继续归档' : '开始归档'" icon="pi pi-download" :disabled="running || batchRetrying || rateWaiting || !loggedIn" @click="start" />
      <Button v-if="running" label="取消" icon="pi pi-times" severity="secondary" outlined @click="cancel" />
      <Button v-if="batchRetrying" :label="batchStopping ? '正在停止…' : '停止重试'" icon="pi pi-stop" severity="warn" outlined :loading="batchStopping" :disabled="batchStopping" @click="stopBatchRetry" />
    </div>
  </section>

  <section v-if="skips.length" class="surface-card task-skips">
    <div class="task-skips-heading"><div><span><i class="pi pi-exclamation-triangle" /></span><div><p class="section-kicker">SKIPPED REQUESTS</p><h3>异常跳过列表</h3></div></div><small>异常位置不会阻塞后续归档，可逐条或批量重试。</small></div>
    <p v-if="skipNotice" class="task-skip-notice" :class="{ 'is-busy': batchRetrying }"><i :class="batchRetrying ? 'pi pi-spin pi-spinner' : 'pi pi-info-circle'" />{{ skipNotice }}</p>
    <div class="task-skip-toolbar">
      <div class="task-skip-filters" role="tablist" aria-label="按恢复状态筛选">
        <button v-for="option in filterOptions" :key="option.value" type="button" role="tab" class="task-skip-filter" :class="{ 'is-active': skipFilter === option.value }" :aria-selected="skipFilter === option.value" @click="skipFilter = option.value">
          {{ option.label }}<small>{{ filterCount(option.value) }}</small>
        </button>
      </div>
      <div class="task-skip-toolbar-actions">
        <Button v-if="pendingSkips.length" :label="batchRetrying ? '批量重试中…' : `一键重试（${pendingSkips.length}）`" icon="pi pi-replay" size="small" :loading="batchRetrying" :disabled="running || batchRetrying || retryingId !== undefined" @click="retryAllPending" />
        <Button v-if="resolvedSkips.length" label="清理已恢复" icon="pi pi-trash" size="small" severity="secondary" text :loading="clearingResolved" :disabled="running || batchRetrying || clearingResolved || retryingId !== undefined" @click="clearResolved" />
      </div>
    </div>
    <div class="task-skip-list" role="list">
      <p v-if="!filteredSkips.length" class="task-skip-empty">当前筛选下暂无记录。</p>
      <article v-for="item in filteredSkips" :key="item.id" class="task-skip-item" :class="{ 'is-resolved': item.resolvedAt }">
        <div class="task-skip-state"><span><i :class="item.resolvedAt ? 'pi pi-check' : 'pi pi-forward'" /></span></div>
        <div class="task-skip-copy">
          <div><strong>第 {{ item.pageNumber }} 页 · offset {{ offsetLabel(item) }}</strong><Tag :value="item.resolvedAt ? '已恢复' : '待重试'" :severity="item.resolvedAt ? 'success' : 'warn'" /></div>
          <p>{{ item.error }}</p>
          <small>跳过于 {{ formatTime(item.skippedAt) }}<template v-if="item.retryCount"> · 已重试 {{ item.retryCount }} 次 · 最近 {{ formatTime(item.lastRetryAt) }}</template><template v-if="item.resolvedAt"> · 恢复 {{ item.recoveredRecords }} 条</template></small>
        </div>
        <Button :label="retryingId === item.id ? '重试中…' : item.resolvedAt ? '已恢复' : '单独重试'" icon="pi pi-refresh" size="small" outlined :loading="retryingId === item.id" :disabled="running || batchRetrying || Boolean(item.resolvedAt) || retryingId !== undefined" @click="retrySkip(item)" />
      </article>
    </div>
  </section>

  <section class="surface-card task-tips">
    <div class="task-tips-heading"><span><i class="pi pi-info-circle" /></span><h4>温馨提示</h4></div>
    <ul>
      <li>空间内容的获取基于 QQ 空间的<strong>互动列表</strong>来获取。没有被点赞或评论过的动态无法被恢复。</li>
      <li>出现<strong>频繁提示</strong>时建议换个时间再继续。程序支持<strong>断点续传</strong>，可以接着上次的进度继续归档。</li>
      <li>归档过程中<strong>不要切换 QQ 客户端账号</strong>，否则可能有冻结风险。</li>
    </ul>
  </section>
</template>
