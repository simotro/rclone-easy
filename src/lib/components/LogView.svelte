<script lang="ts">
  import Icon from "./Icon.svelte";
  import { t } from "$lib/i18n";

  let { text }: { text: string } = $props();

  let copied = $state(false);
  let resetTimer: ReturnType<typeof setTimeout> | undefined;

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
      clearTimeout(resetTimer);
      resetTimer = setTimeout(() => (copied = false), 1500);
    } catch (error) {
      // Nessun fallback silenzioso da inventare per un ambiente senza
      // clipboard: il pulsante semplicemente non fa nulla di visibile,
      // meglio di un errore poco chiaro per un'azione così secondaria.
    }
  }
</script>

<div class="log-view-wrapper">
  <button type="button" class="copy-log-button" onclick={copy}>
    {#if copied}
      ✓ {$t("remoteRow.logCopied")}
    {:else}
      <Icon kind="copy" />
      {$t("remoteRow.copyLog")}
    {/if}
  </button>
  <pre class="log-view">{text}</pre>
</div>

<style>
.log-view-wrapper {
  position: relative;
}

.copy-log-button {
  position: absolute;
  top: 0.5em;
  right: 0.5em;
  display: inline-flex;
  align-items: center;
  gap: 0.35em;
  padding: 0.3em 0.6em;
  font-size: 0.78em;
  border-radius: 6px;
  border: 1px solid var(--border-color-subtle);
  background-color: var(--bg-surface);
  color: var(--text-color);
  cursor: pointer;
  box-shadow: var(--shadow-sm);
}

.copy-log-button :global(svg) {
  width: 1.1em;
  height: 1.1em;
}

.log-view {
  margin: 0;
  max-height: 40vh;
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: monospace;
  font-size: 0.8em;
  background-color: var(--surface-tint);
  border-radius: 6px;
  padding: 0.7em 0.9em;
  /* Spazio per il pulsante copia sovrapposto in alto a destra, non deve
     coprire la prima riga di log. */
  padding-top: 2.6em;
}
</style>
