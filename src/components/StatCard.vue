<script setup lang="ts">
defineProps<{ label: string; value: string; hint: string; icon: string; tone?: string }>();
</script>

<template>
  <article class="stat-card" :class="`has-tone-${tone ?? 'blue'}`">
    <span class="stat-icon" :class="`tone-${tone ?? 'blue'}`">
      <i :class="icon" />
    </span>
    <div class="stat-content">
      <div class="stat-label-row">
        <p>{{ label }}</p>
        <span class="stat-pulse-dot" />
      </div>
      <strong>{{ value }}</strong>
      <small>{{ hint }}</small>
    </div>
  </article>
</template>

<style scoped>
.stat-content {
  min-width: 0;
  flex: 1;
}

.stat-label-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.stat-pulse-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
  opacity: 0.4;
  box-shadow: 0 0 8px currentColor;
  animation: pulse-glow 2s infinite ease-in-out;
}

@keyframes pulse-glow {
  0%, 100% { transform: scale(1); opacity: 0.4; }
  50% { transform: scale(1.4); opacity: 0.9; }
}
</style>
