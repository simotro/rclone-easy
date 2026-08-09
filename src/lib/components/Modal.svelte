<script lang="ts">
  import type { Snippet } from "svelte";

  let { open = $bindable(false), title, children }: { open: boolean; title: string; children: Snippet } = $props();

  function onKeydown(event: KeyboardEvent) {
    if (open && event.key === "Escape") open = false;
  }
</script>

<svelte:window onkeydown={onKeydown} />

<!--
  Overlay disegnato a mano invece di <dialog>/showModal(): su WebKitGTK (il
  motore usato da Tauri su Linux) la promozione al "top layer" di <dialog>
  si è vista rompere sia i colori del tema (il dialog ignorava le custom
  property scure ereditate da :root, restando chiaro anche a tema scuro
  impostato) sia il posizionamento di ThemeToggle (position: fixed,
  sganciato dal suo angolo mentre un dialog era aperto) — bug del motore
  legato al top layer, non riproducibile in Chromium. Un overlay normale
  resta nel flusso/nell'albero di stacking ordinario, quindi non può
  soffrire di quella classe di bug.
-->
{#if open}
  <div class="backdrop">
    <div class="dialog" role="dialog" aria-modal="true">
      <div class="header">
        <h2>{title}</h2>
        <button type="button" class="close-button" onclick={() => (open = false)} aria-label="Chiudi">✕</button>
      </div>
      <div class="content">
        {@render children()}
      </div>
    </div>
  </div>
{/if}

<style>
.backdrop {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
  background-color: rgba(0, 0, 0, 0.45);
}

.dialog {
  display: flex;
  flex-direction: column;
  border-radius: 10px;
  width: min(30em, 92vw);
  max-height: 85vh;
  color: var(--text-color);
  background-color: var(--bg-page);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.25);
}

.header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 1em 1.2em;
  border-bottom: 1px solid var(--border-color-subtle);
  flex-shrink: 0;
}

h2 {
  margin: 0;
  font-size: 1.1em;
}

.close-button {
  background: none;
  border: none;
  box-shadow: none;
  font-size: 1.1em;
  padding: 0.2em 0.5em;
  cursor: pointer;
  color: inherit;
}

.content {
  padding: 1.1em 1.2em;
  overflow-y: auto;
  min-height: 0;
}
</style>
