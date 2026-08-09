<script lang="ts">
  let { kind }: { kind: "mount" | "backup" | "bisync" | "edit" | "delete" } = $props();
</script>

{#if kind === "mount"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
    <rect x="2" y="6" width="20" height="12" rx="2" />
    <line x1="2" y1="12" x2="22" y2="12" />
    <circle cx="6" cy="15" r="1.1" fill="currentColor" stroke="none" />
    <circle cx="9.8" cy="15" r="1.1" fill="currentColor" stroke="none" />
  </svg>
{:else if kind === "backup"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
    <path d="M3 12a9 9 0 0 1 15.3-6.4L21 8" />
    <polyline points="21 3 21 8 16 8" />
  </svg>
{:else if kind === "bisync"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
    <path d="M4 7h13l-3-3" />
    <path d="M20 17H7l3 3" />
  </svg>
{:else if kind === "edit"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
    <path d="M12 20h9" />
    <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" />
  </svg>
{:else if kind === "delete"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
    <polyline points="3 6 5 6 21 6" />
    <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
    <path d="M10 11v6" />
    <path d="M14 11v6" />
    <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
  </svg>
{/if}

<style>
svg {
  width: 1.3em;
  height: 1.3em;
  display: block;
  flex-shrink: 0;
}
</style>
