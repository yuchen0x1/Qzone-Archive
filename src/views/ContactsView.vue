<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import Button from "primevue/button";
import InputText from "primevue/inputtext";
import Select from "primevue/select";
import { listInteractors, type Interactor } from "../utils/qzone";

const contacts = ref<Interactor[]>([]);
const loading = ref(false);
const error = ref("");
const query = ref("");
const sortBy = ref<"total" | "likes" | "comments" | "lastAt">("total");

type SortOption = { label: string; value: typeof sortBy.value };
const sortOptions: SortOption[] = [
  { label: "互动最多", value: "total" },
  { label: "点赞最多", value: "likes" },
  { label: "评论最多", value: "comments" },
  { label: "最近互动", value: "lastAt" },
];

const filtered = computed(() => {
  const key = query.value.trim().toLowerCase();
  let list = contacts.value;
  if (key) {
    list = list.filter(
      (c) => c.nickname?.toLowerCase().includes(key) || c.uin.includes(key),
    );
  }
  return [...list].sort((a, b) => {
    switch (sortBy.value) {
      case "likes":
        return b.likes - a.likes || b.total - a.total;
      case "comments":
        return b.comments - a.comments || b.total - a.total;
      case "lastAt":
        return b.lastAt - a.lastAt;
      default:
        return b.total - a.total;
    }
  });
});

const formatTime = (seconds: number) =>
  seconds
    ? new Intl.DateTimeFormat("zh-CN", { dateStyle: "medium" }).format(
        new Date(seconds * 1000),
      )
    : "";

async function load() {
  loading.value = true;
  error.value = "";
  try {
    contacts.value = await listInteractors();
  } catch (reason) {
    error.value = String(reason);
  } finally {
    loading.value = false;
  }
}

onMounted(load);
</script>

<template>
  <div class="contacts-page">
    <section class="surface-card archive-header">
      <div class="archive-header-copy">
        <span class="archive-header-icon"><i class="pi pi-users" /></span>
        <div>
          <p class="section-kicker">CONTACTS</p>
          <h2>联系人</h2>
          <p>所有与你有过互动的联系人，共 {{ contacts.length }} 位</p>
        </div>
      </div>
      <div class="archive-header-actions">
        <Button icon="pi pi-refresh" label="刷新" severity="secondary" text :loading="loading" @click="load" />
      </div>
    </section>

    <section class="contacts-toolbar surface-card">
      <div class="search-box">
        <i class="pi pi-search" />
        <InputText
          v-model="query"
          placeholder="搜索昵称或 QQ 号"
        />
      </div>
      <Select
        v-model="sortBy"
        :options="sortOptions"
        option-label="label"
        option-value="value"
        class="contacts-sort"
        placeholder="排序方式"
      />
    </section>

    <p v-if="error" class="archive-error">
      <i class="pi pi-exclamation-circle" />{{ error }}
    </p>

    <div v-if="loading" class="contacts-loading">
      <i class="pi pi-spin pi-spinner" />
      <p>正在加载联系人…</p>
    </div>

    <section v-else-if="filtered.length" class="contacts-grid">
      <article
        v-for="c in filtered"
        :key="c.uin"
        class="surface-card contact-card"
      >
        <img
          :src="`https://qlogo2.store.qq.com/qzone/${c.uin}/${c.uin}/50`"
          referrerpolicy="no-referrer"
          loading="lazy"
          class="contact-avatar"
          :alt="c.nickname"
          @error="(e) => ((e.target as HTMLImageElement).style.visibility = 'hidden')"
        />
        <div class="contact-body">
          <strong>{{ c.nickname || c.uin }}</strong>
          <small v-if="c.nickname && c.nickname !== c.uin"
            >QQ {{ c.uin }}</small
          >
          <div class="contact-stats">
            <span title="点赞"><i class="pi pi-heart" />{{ c.likes }}</span>
            <span title="评论"><i class="pi pi-comment" />{{ c.comments }}</span>
            <span title="最后互动">{{ formatTime(c.lastAt) }}</span>
          </div>
        </div>
        <div class="contact-total">
          <strong>{{ c.total }}</strong>
          <small>次互动</small>
        </div>
      </article>
    </section>

    <section v-else class="surface-card empty-state page-empty">
      <span><i class="pi pi-users" /></span>
      <h2>{{ query ? "没有匹配的联系人" : "暂无联系人数据" }}</h2>
      <p>{{ query ? "尝试更换搜索关键词。" : "请先前往任务页执行归档，归档后这里会显示与你互动过的联系人。" }}</p>
    </section>
  </div>
</template>
