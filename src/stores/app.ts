import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";

const DARK_MODE_KEY = "qzone-dark-mode";
const PRIVACY_MODE_KEY = "qzone_privacy_mode";

function resolveInitialDark(): boolean {
  const stored = localStorage.getItem(DARK_MODE_KEY);
  if (stored === "true") return true;
  if (stored === "false") return false;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
}

export const useAppStore = defineStore("app", () => {
  const darkMode = ref(resolveInitialDark());
  const privacyMode = ref(localStorage.getItem(PRIVACY_MODE_KEY) === "true");
  const sidebarCollapsed = ref(true);
  const themeIcon = computed(() => (darkMode.value ? "pi pi-sun" : "pi pi-moon"));
  const privacyIcon = computed(() => (privacyMode.value ? "pi pi-eye-slash" : "pi pi-eye"));

  watch(darkMode, (val) => {
    localStorage.setItem(DARK_MODE_KEY, String(val));
  }, { immediate: true });

  watch(privacyMode, (val) => {
    localStorage.setItem(PRIVACY_MODE_KEY, String(val));
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
