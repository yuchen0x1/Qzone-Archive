import { createRouter, createWebHashHistory } from "vue-router";

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    {
      path: "/",
      name: "dashboard",
      component: () => import("../views/DashboardView.vue"),
      meta: { title: "概览" },
    },
    {
      path: "/archives",
      name: "archives",
      component: () => import("../views/ArchivesView.vue"),
      meta: { title: "归档内容" },
    },
    {
      path: "/contacts",
      name: "contacts",
      component: () => import("../views/ContactsView.vue"),
      meta: { title: "联系人" },
    },
    {
      path: "/media",
      name: "media",
      component: () => import("../views/MediaView.vue"),
      meta: { title: "媒体" },
    },
    {
      path: "/albums",
      name: "albums",
      component: () => import("../views/AlbumsView.vue"),
      meta: { title: "相册备份" },
    },
    {
      path: "/tasks",
      name: "tasks",
      component: () => import("../views/TasksView.vue"),
      meta: { title: "归档任务" },
    },
    {
      path: "/recycle-bin",
      name: "recycle-bin",
      component: () => import("../views/RecycleBinView.vue"),
      meta: { title: "相册回收站" },
    },
    {
      path: "/settings",
      name: "settings",
      component: () => import("../views/SettingsView.vue"),
      meta: { title: "设置" },
    },
    {
      path: "/about",
      name: "about",
      component: () => import("../views/AboutView.vue"),
      meta: { title: "关于" },
    },
  ],
});

router.afterEach((to) => {
  document.title = to.meta.title ? `${to.meta.title} - 空间归档` : "空间归档";
});

export default router;
