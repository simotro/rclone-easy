<script lang="ts">
  let { kind }: { kind: "mount" | "backup" | "bisync" | "edit" | "delete" | "eye" | "eye-off" | "quit" } = $props();
</script>

{#if kind === "mount"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
    <rect x="2.5" y="6" width="19" height="12" rx="3.5" />
    <path d="M2.5 12.5h19" />
    <circle cx="6.3" cy="15.2" r="1" fill="currentColor" stroke="none" />
    <circle cx="9.6" cy="15.2" r="1" fill="currentColor" stroke="none" />
  </svg>
{:else if kind === "backup"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
    <path d="M7 18a4.5 4.5 0 0 1-1-8.9 6 6 0 0 1 11.6-2A4.5 4.5 0 0 1 17 18H7Z" />
    <path d="M12 9.5v7" />
    <path d="M9 12.2 12 9.3l3 2.9" />
  </svg>
{:else if kind === "bisync"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
    <path d="M6.5 8.5A6.5 6.5 0 0 1 18 6.2" />
    <path d="M17.5 15.5A6.5 6.5 0 0 1 6 17.8" />
    <path d="M17.3 3.8 18 6.2l-2.5.6" />
    <path d="M6.7 20.2 6 17.8l2.5-.6" />
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
{:else if kind === "eye"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
    <path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7-10-7-10-7Z" />
    <circle cx="12" cy="12" r="3" />
  </svg>
{:else if kind === "eye-off"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
    <path d="M3 3l18 18" />
    <path d="M10.6 5.2A10.6 10.6 0 0 1 12 5c6.4 0 10 7 10 7a15.8 15.8 0 0 1-4 4.6M6.6 6.6C4 8.3 2 12 2 12s3.6 7 10 7a9.7 9.7 0 0 0 4.4-1" />
    <path d="M9.9 9.9a3 3 0 0 0 4.2 4.2" />
  </svg>
{:else if kind === "quit"}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
    <line x1="6" y1="6" x2="18" y2="18" />
    <line x1="18" y1="6" x2="6" y2="18" />
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
