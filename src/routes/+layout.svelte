<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import "$lib/shared-styles.css";
  import { initTheme } from "$lib/theme.svelte";
  import ThemeToggle from "$lib/components/ThemeToggle.svelte";
  import WindowControls from "$lib/components/WindowControls.svelte";
  import UnlockScreen from "$lib/components/UnlockScreen.svelte";

  let { children } = $props();

  initTheme();

  // "loading" evita un lampo della home prima di sapere se la config è
  // protetta — vedi rcd::needs_unlock/locked_state per il perché questo
  // può essere vero all'avvio (config_password.rs).
  let unlockState = $state<"loading" | "locked" | "unlocked">("loading");

  $effect(() => {
    invoke<boolean>("needs_unlock")
      .then((needsUnlock) => (unlockState = needsUnlock ? "locked" : "unlocked"))
      .catch(() => (unlockState = "unlocked"));
  });
</script>

<div class="app-shell">
  <!-- Unica area trascinabile della finestra (niente più barra del titolo
       nativa, vedi tauri.conf.json/tray.rs): una striscia dedicata invece
       di marcare pezzi sparsi dell'interfaccia — tentativo precedente,
       abbandonato perché su WebKitGTK marcare un antenato di un modal
       (`.remote-list`) rompeva il posizionamento `position: fixed` dei
       suoi discendenti (il modal si centrava sulla lista invece che sulla
       finestra). Una striscia isolata, senza contenuto applicativo sotto,
       non corre questo rischio. `WindowControls` (nascondi in tray) sempre
       presente, indipendentemente dallo stato di sblocco — l'unico modo,
       oltre alla tray, di agire sulla finestra (un secondo pulsante per
       l'uscita vera creava confusione, tolto: resta solo in "Esci" nel
       menu della tray). Il tema invece resta disponibile solo a sblocco
       avvenuto, come già prima — e va prima di `WindowControls` nel
       markup: `.title-bar` è allineata a destra (`justify-content:
       flex-end`), quindi l'ordine nel DOM è l'ordine visivo da sinistra a
       destra. -->
  <div class="title-bar" data-tauri-drag-region>
    {#if unlockState === "unlocked"}
      <ThemeToggle />
    {/if}
    <WindowControls />
  </div>

  <div class="app-body">
    {#if unlockState === "loading"}
      <!-- Niente da mostrare ancora: evita di far vedere per un istante la home
           (con le sue chiamate che fallirebbero comunque finché bloccata) prima
           di sapere se serve la password. -->
    {:else if unlockState === "locked"}
      <UnlockScreen onUnlocked={() => (unlockState = "unlocked")} />
    {:else}
      {@render children()}
    {/if}
  </div>
</div>

<style>
.app-shell {
  display: flex;
  flex-direction: column;
  min-height: 100vh;
  /* Senza barra del titolo nativa (`decorations: false`, vedi
     tao_wayland_x_button_bug — scelta finale, non provvisoria: il bug del
     pulsante X su KDE/Wayland è fisso solo in tao >= 0.36, non ancora
     raggiungibile dalla versione di Tauri usata qui) la finestra non ha più
     un bordo/ombra propri forniti dal window manager, quindi si confonde
     con lo sfondo del desktop dietro di essa (segnalato da Simone
     l'11/8/2026). Un bordo sottile disegnato da noi risolve senza
     riaprire la questione delle decorazioni. `box-sizing: border-box`
     tiene il bordo dentro il budget di `min-height: 100vh` invece di
     sommarcisi sopra — stessa cautela già presa per il margine di default
     di `body` (vedi shared-styles.css), che causava una scrollbar
     ingiustificata per un motivo simile.
  */
  box-sizing: border-box;
  border: 1px solid var(--border-color);
  box-shadow: 0 4px 24px rgba(0, 0, 0, 0.35);
}

.title-bar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 0.5em;
  padding: 0.5em 0.8em;
  background-color: var(--bg-surface);
  border-bottom: 1px solid var(--border-color-subtle);
}

.app-body {
  flex: 1;
  min-height: 0;
}
</style>
