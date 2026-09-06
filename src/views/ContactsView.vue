<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import Button from "primevue/button";
import InputText from "primevue/inputtext";
import Select from "primevue/select";
import { listFriendContacts, listInteractors, syncFriendList, type FriendContact, type Interactor } from "../utils/qzone";

const contacts = ref<Interactor[]>([]);
const friends = ref<FriendContact[]>([]);
const mode = ref<"interact" | "friends">("interact");
const loading = ref(false);
const syncing = ref(false);
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

const filteredInteractors = computed(() => {
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

const filteredFriends = computed(() => {
  const key = query.value.trim().toLowerCase();
  const list = friends.value;
  return key
    ? list.filter((c) => c.nickname?.toLowerCase().includes(key) || c.uin.includes(key) || c.remark?.toLowerCase().includes(key))
    : list;
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
    if (mode.value === "friends") {
      friends.value = await listFriendContacts();
    } else {
      contacts.value = await listInteractors();
    }
  } catch (reason) {
    error.value = String(reason);
  } finally {
    loading.value = false;
  }
}

async function syncFriends() {
  if (syncing.value) return;
  syncing.value = true;
  error.value = "";
  try {
    friends.value = await syncFriendList();
  } catch (reason) {
    error.value = `同步好友列表失败：${String(reason)}`;
  } finally {
    syncing.value = false;
  }
}

watch(mode, () => {
  query.value = "";
  void load();
});

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
          <p v-if="mode === 'interact'">所有与你有过互动的联系人，共 {{ contacts.length }} 位</p>
          <p v-else>QQ 空间好友列表，共 {{ friends.length }} 位</p>
        </div>
      </div>
      <div class="archive-header-actions">
        <Button v-if="mode === 'friends'" icon="pi pi-cloud-download" label="同步好友列表" severity="secondary" text :loading="syncing" @click="syncFriends" />
        <Button icon="pi pi-refresh" label="刷新" severity="secondary" text :loading="loading" @click="load" />
      </div>
    </section>

    <section class="contacts-toolbar surface-card">
      <nav class="contacts-mode-tabs" aria-label="联系人类型">
        <button type="button" class="contacts-mode-tab" :class="{ 'is-active': mode === 'interact' }" @click="mode = 'interact'">互动联系人</button>
        <button type="button" class="contacts-mode-tab" :class="{ 'is-active': mode === 'friends' }" @click="mode = 'friends'">QQ 好友</button>
      </nav>
      <div class="search-box">
        <i class="pi pi-search" />
        <InputText
          v-model="query"
          placeholder="搜索昵称或 QQ 号"
        />
      </div>
      <Select
        v-if="mode === 'interact'"
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

    <section v-else-if="filteredInteractors.length && mode === 'interact'" class="contacts-grid">
      <article
        v-for="c in filteredInteractors"
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

    <section v-else-if="filteredFriends.length && mode === 'friends'" class="contacts-grid">
      <article v-for="f in filteredFriends" :key="f.uin" class="surface-card contact-card">
        <img
          :src="`https://qlogo2.store.qq.com/qzone/${f.uin}/${f.uin}/50`"
          referrerpolicy="no-referrer"
          loading="lazy"
          class="contact-avatar"
          :alt="f.nickname"
          @error="(e) => ((e.target as HTMLImageElement).style.visibility = 'hidden')"
        />
        <div class="contact-body">
          <strong>{{ f.nickname || f.uin }}</strong>
          <small v-if="f.remark && f.remark !== f.nickname">备注：{{ f.remark }}</small>
          <small>QQ {{ f.uin }}</small>
        </div>
      </article>
    </section>

    <section v-else class="surface-card empty-state page-empty">
      <span><i class="pi pi-users" /></span>
      <h2>{{ query ? "没有匹配的联系人" : mode === "friends" ? "还没有好友列表数据" : "暂无联系人数据" }}</h2>
      <p v-if="mode === 'friends'">{{ query ? "尝试更换搜索关键词。" : "点击右上角「同步好友列表」，把 QQ 空间好友保存到本地。" }}</p>
      <p v-else>{{ query ? "尝试更换搜索关键词。" : "请先前往任务页执行归档，归档后这里会显示与你互动过的联系人。" }}</p>
    </section>  </div>
</template>
