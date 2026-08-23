<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { goto } from "$app/navigation";
  import "$lib/shared-styles.css";
  import { initTheme } from "$lib/theme.svelte";
  import "$lib/i18n";
  import ThemeToggle from "$lib/components/ThemeToggle.svelte";
  import LanguageToggle from "$lib/components/LanguageToggle.svelte";
  import AboutButton from "$lib/components/AboutButton.svelte";
  import UpdateButton from "$lib/components/UpdateButton.svelte";
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

  // Cliccando "Configura" o una voce di avviso nel menu della tray, il
  // backend porta la finestra in primo piano ed emette questo evento — a
  // livello di layout (non più per-riga in RemoteRow.svelte) perché ora
  // naviga direttamente alla pagina del remote, indipendentemente da quale
  // pagina è aperta al momento. Una voce di avviso porta dritti alla
  // Cronologia (dove si vede cosa è fallito) invece che alla scheda
  // Configura di default — vedi tray.rs::focus_remote.
  $effect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<{ remote: string; openHistory: boolean }>("rclone-easy://tray-focus-remote", (event) => {
      const path = `/remote/${encodeURIComponent(event.payload.remote)}`;
      goto(event.payload.openHistory ? `${path}?tab=cronologia` : path);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
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
       menu della tray). Lingua, tema e informazioni sull'app invece
       restano disponibili solo a sblocco avvenuto, come già prima — e
       vanno prima di `WindowControls` nel markup: `.title-bar` è
       allineata a destra (`justify-content: flex-end`), quindi l'ordine
       nel DOM è l'ordine visivo da sinistra a destra (lingua, poi tema,
       poi informazioni sull'app, poi controlli finestra; il pulsante
       Impostazioni vive invece sulla home, accanto ad "Aggiungi remote" —
       vedi +page.svelte). -->
  <div class="title-bar" data-tauri-drag-region>
    {#if unlockState === "unlocked"}
      <LanguageToggle />
      <ThemeToggle />
      <UpdateButton />
      <AboutButton />
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
  /* `height` (non `min-height`): con solo un minimo, una pagina più alta
     della finestra faceva crescere l'intero shell oltre i 100vh, ed era il
     documento a scorrere — trascinando con sé `.title-bar` (bug segnalato
     da Simone il 19/8/2026: finestra non più raggiungibile per spostarla o
     chiuderla una volta scorsa via). Bloccando l'altezza qui e lasciando
     scorrere solo `.app-body` sotto, la barra resta sempre visibile e
     cliccabile. */
  height: 100vh;
  overflow: hidden;
  /* Senza barra del titolo nativa (`decorations: false` — scelta
     definitiva: un bug di tao su KDE/Wayland rende il pulsante X non
     cliccabile con le decorazioni native attive, fix upstream non ancora
     disponibile nella versione di Tauri usata qui) la finestra non ha più
     un bordo/ombra propri forniti dal window manager, e si confonde con lo
     sfondo del desktop dietro di essa. Un bordo sottile disegnato da noi
     risolve senza riaprire la questione delle decorazioni.
     `box-sizing: border-box` tiene il bordo dentro il budget di
     `height: 100vh` invece di sommarcisi sopra — stessa cautela già
     presa per il margine di default di `body` (vedi shared-styles.css),
     che causava una scrollbar ingiustificata per un motivo simile. */
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
  overflow-y: auto;
}
</style>
