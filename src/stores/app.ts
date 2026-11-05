import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";

export const useAppStore = defineStore("app", () => {
  const darkMode = ref(false);
  const privacyMode = ref(localStorage.getItem("qzone_privacy_mode") === "true");
  const sidebarCollapsed = ref(true);
  const themeIcon = computed(() => (darkMode.value ? "pi pi-sun" : "pi pi-moon"));
  const privacyIcon = computed(() => (privacyMode.value ? "pi pi-eye-slash" : "pi pi-eye"));

  watch(privacyMode, (val) => {
    localStorage.setItem("qzone_privacy_mode", String(val));
    if (val) {
      document.documentElement.classList.add("privacy-mode");
    } else {
      document.documentElement.classList.remove("privacy-mode");
    }
  }, { immediate: true });

  function toggleTheme() {
    darkMode.value = !darkMode.value;
  }

  function togglePrivacy() {
    privacyMode.value = !privacyMode.value;
  }

  function toggleSidebar() {
    sidebarCollapsed.value = !sidebarCollapsed.value;
  }

  return {
    darkMode,
    privacyMode,
    sidebarCollapsed,
    themeIcon,
    privacyIcon,
    toggleTheme,
    togglePrivacy,
    toggleSidebar,
  };
});
