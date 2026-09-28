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

  // La finestra viene distrutta alla chiusura (libera la webview) e
  // ricreata da tray.rs: gli eventi emessi subito dopo la creazione
  // (Configura, Impostazioni, Aggiornamento) aspettano questo segnale, dato
  // quando la pagina e i suoi ascoltatori sono attivi — vedi
  // tray.rs::emit_to_ui. Il ritardo lascia il tempo ai componenti figli di
  // registrare i loro `listen`, asincroni.
  $effect(() => {
    if (unlockState === "loading") return;
    const id = setTimeout(() => invoke("frontend_ready").catch(() => {}), 400);
    return () => clearTimeout(id);
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
  <!-- Striscia dei controlli globali (lingua, tema, aggiornamenti,
       informazioni sull'app), presente su ogni pagina ma solo a sblocco
       avvenuto. L'ordine nel DOM è l'ordine visivo da sinistra a destra,
       perché la striscia è allineata a destra. Il pulsante Impostazioni vive
       invece sulla home, accanto ad "Aggiungi remote" (vedi +page.svelte). -->
  {#if unlockState === "unlocked"}
    <div class="toolbar">
      <LanguageToggle />
      <ThemeToggle />
      <UpdateButton />
      <AboutButton />
    </div>
  {/if}

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
     documento a scorrere, trascinando con sé `.toolbar`. Bloccando
     l'altezza qui e lasciando scorrere solo `.app-body` sotto, la striscia
     resta sempre visibile. */
  height: 100vh;
  overflow: hidden;
}

.toolbar {
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
