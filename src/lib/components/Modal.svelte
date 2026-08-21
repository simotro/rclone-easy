<script lang="ts">
  import type { Snippet } from "svelte";
  import { t } from "$lib/i18n";

  let {
    open = $bindable(false),
    title,
    children,
    elevated = false,
  }: { open: boolean; title: string; children: Snippet; elevated?: boolean } = $props();

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
  <div class="backdrop" class:elevated>
    <div class="dialog" role="dialog" aria-modal="true">
      <div class="header">
        <h2>{title}</h2>
        <button type="button" class="close-button" onclick={() => (open = false)} aria-label={$t("common.close")}>✕</button>
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

/* Per un modal che può restare aperto SOPRA un altro (es. un avviso di
   conferma che nidifica dentro un modal di configurazione già aperto) —
   senza questo, due `.backdrop` con lo stesso z-index si contendono lo
   stacking in base al solo ordine nel DOM: fragile, si rompe silenziosamente
   se qualcuno riordina i componenti in futuro. Trovato con un test Playwright
   reale (21/8/2026): il modal di avviso "home directory" del bisync esisteva
   nel DOM ma i click venivano intercettati dal modal di configurazione
   sottostante, che nel markup comparivano dopo e quindi vinceva lo stacking. */
.backdrop.elevated {
  z-index: 1100;
}

.dialog {
  display: flex;
  flex-direction: column;
  border-radius: 10px;
  width: min(30em, 92vw);
  /* Alzato da 85vh: con la finestra di default (vedi tauri.conf.json,
     780px di altezza) lasciava troppo poco spazio verticale al contenuto
     dei form più lunghi, costringendo a scorrere anche solo per vedere i
     pulsanti di azione in fondo. */
  max-height: 94vh;
  color: var(--text-color);
  background-color: var(--bg-page);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.25);
}

.header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 0.8em 1.1em;
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
  padding: 0.9em 1.1em;
  overflow-y: auto;
  min-height: 0;
}

/* I paragrafi (hint/errori/avvisi) dentro un modal usano il margine di
   default del browser (1em sopra e sotto) se non azzerato: dentro un
   contenitore flex con `gap` (vedi `.modal-form` in RemoteRow.svelte) quel
   margine non collassa come farebbe in un flusso normale, si somma al gap
   riga per riga — risultato uno spazio verticale doppio del previsto tra
   un'etichetta/hint e il campo sotto. Azzerato qui una volta per tutti i
   consumatori di Modal, così il `gap` del contenitore resta l'unica fonte
   di spaziatura, facile da tarare in un punto solo. `:global` perché il
   contenuto è renderizzato dal chiamante (`{@render children()}`), fuori
   dallo scoping automatico degli stili di questo componente. */
.content :global(p) {
  margin: 0;
}
</style>
