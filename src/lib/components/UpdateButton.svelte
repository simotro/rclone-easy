<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import Modal from "./Modal.svelte";
  import { t } from "$lib/i18n";
  import { updateState, checkForUpdates, skipUpdate } from "$lib/updates.svelte";

  const DOWNLOAD_ICON = `<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M12 3v12m0 0-4-4m4 4 4-4M5 21h14" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`;

  const RELEASES_URL = "https://github.com/simotro/rclone-easy/releases/latest";

  let open = $state(false);
  let installing = $state(false);
  let installError = $state<string | null>(null);
  // Percentuale di avanzamento del download: nota solo se il server manda
  // Content-Length nell'evento "Started" (di solito sì, per GitHub
  // Releases) — altrimenti resta null e la UI mostra un indicatore
  // indeterminato invece di una percentuale inventata.
  let downloadTotal = $state<number | null>(null);
  let downloadReceived = $state(0);
  // Vero da quando il file è già stato scaricato e sostituito su disco, in
  // attesa che l'utente scelga di riavviare — prima l'app si riavviava da
  // sola non appena finiva il download, senza che l'utente potesse scegliere
  // il momento (Simone, 21/8/2026).
  let readyToRestart = $state(false);
  let restarting = $state(false);

  let downloadPercent = $derived(downloadTotal ? Math.min(100, Math.round((downloadReceived / downloadTotal) * 100)) : null);

  // Un solo controllo automatico all'avvio dell'app (non ripetuto ad ogni
  // montaggio del componente, che con questa disposizione resta comunque
  // montato una sola volta) — un controllo manuale resta disponibile dalle
  // impostazioni (SettingsButton), che riusa lo stesso stato condiviso.
  $effect(() => {
    checkForUpdates();
  });

  // Cliccando la voce "⬆ Aggiornamento disponibile" nel menu della tray, o
  // "Mostra/Nascondi finestra" quando c'è un aggiornamento in sospeso, il
  // backend porta la finestra in primo piano ed emette questo evento — vedi
  // tray.rs::open_update/toggle_main_window. Stesso schema di
  // SettingsButton.svelte per OPEN_SETTINGS_EVENT.
  $effect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen("rclone-easy://open-update", () => (open = true)).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  });

  async function installNow() {
    const s = updateState();
    if (s.status !== "available") return;
    installing = true;
    installError = null;
    downloadTotal = null;
    downloadReceived = 0;
    try {
      await s.update.downloadAndInstall((event) => {
        if (event.event === "Started") downloadTotal = event.data.contentLength ?? null;
        else if (event.event === "Progress") downloadReceived += event.data.chunkLength;
      });
      // L'AppImage/l'installer NSIS sono già stati sostituiti sul disco a
      // questo punto, ma il riavvio vero e proprio parte solo quando lo
      // conferma l'utente (restartNow), non in automatico.
      installing = false;
      readyToRestart = true;
    } catch (error) {
      installError = String(error);
      installing = false;
    }
  }

  async function restartNow() {
    restarting = true;
    // restart_app (non il semplice relaunch del plugin updater) fa
    // ripartire l'app passando comunque dal nostro spegnimento pulito del
    // demone rclone rcd, vedi lib.rs::restart_app.
    await invoke("restart_app");
  }

  async function openDownloadPage() {
    await invoke("open_url_in_browser", { url: RELEASES_URL });
  }

  // Le note vere e proprie vivono solo sulla pagina della release taggata
  // (generate da GitHub, non replicate in latest.json) — un link reale
  // invece del testo statico "Vedi le note della release su GitHub" che
  // prima non portava da nessuna parte.
  async function openReleaseNotes(version: string) {
    await invoke("open_url_in_browser", { url: `https://github.com/simotro/rclone-easy/releases/tag/v${version}` });
  }
</script>

{#if updateState().status === "available"}
  <button type="button" class="update-trigger" title={$t("update.available")} aria-label={$t("update.available")} onclick={() => (open = true)}>
    {@html DOWNLOAD_ICON}
  </button>

  <Modal bind:open title={$t("update.title")}>
    {@const s = updateState()}
    {#if s.status === "available"}
      <div class="update-content">
        <p>{$t("update.newVersion", { values: { version: s.update.version } })}</p>
        <button type="button" class="link-button" onclick={() => openReleaseNotes(s.update.version)}>
          {$t("update.viewReleaseNotes")}
        </button>
        {#if installError}
          <p class="error">✗ {installError}</p>
        {/if}
        {#if readyToRestart}
          <p class="ready-hint">{$t("update.readyToRestart")}</p>
          <div class="actions">
            <button type="button" class="link-button" onclick={() => (open = false)} disabled={restarting}>{$t("update.later")}</button>
            <button type="button" onclick={restartNow} disabled={restarting}>
              {restarting ? $t("update.restarting") : $t("update.restartNow")}
            </button>
          </div>
        {:else}
          {#if installing}
            <div class="progress-track" role="progressbar" aria-valuenow={downloadPercent ?? undefined} aria-valuemin={0} aria-valuemax={100}>
              <div class="progress-fill" class:indeterminate={downloadPercent === null} style={downloadPercent !== null ? `width: ${downloadPercent}%` : undefined}></div>
            </div>
            <p class="hint">{downloadPercent !== null ? $t("update.downloadingPercent", { values: { percent: downloadPercent } }) : $t("update.installing")}</p>
          {/if}
          <div class="actions">
            <button type="button" class="link-button" onclick={skipUpdate} disabled={installing}>{$t("update.skipVersion")}</button>
            {#if s.installKind === "appimage" || s.installKind === "windows"}
              <button type="button" onclick={installNow} disabled={installing}>
                {installing ? $t("update.installing") : $t("update.installNow")}
              </button>
            {:else}
              <button type="button" onclick={openDownloadPage}>{$t("update.openDownloadPage")}</button>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  </Modal>
{/if}

<style>
.update-trigger {
  background: none;
  border: 1px solid var(--accent);
  box-shadow: none;
  padding: 0.5em;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--accent);
}

.update-trigger:hover {
  color: var(--text-color);
  border-color: var(--text-color);
}

.update-trigger :global(svg) {
  width: 1.3em;
  height: 1.3em;
  display: block;
}

.update-content {
  display: flex;
  flex-direction: column;
  gap: 0.7em;
}

.link-button {
  align-self: flex-start;
  background: none;
  border: none;
  box-shadow: none;
  padding: 0;
  color: var(--accent);
  cursor: pointer;
  font-size: 0.9em;
}

.actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.ready-hint {
  color: var(--text-color);
}

.progress-track {
  width: 100%;
  height: 0.5em;
  border-radius: 100px;
  background-color: var(--surface-tint);
  overflow: hidden;
}

.progress-fill {
  height: 100%;
  border-radius: 100px;
  background-color: var(--accent);
  transition: width 0.2s ease;
}

/* Content-Length non sempre disponibile (dipende dal server) — senza una
   percentuale nota, una barra piena ferma sembrerebbe bloccata: questa si
   anima da sola per mostrare comunque che qualcosa sta succedendo. */
.progress-fill.indeterminate {
  width: 40%;
  animation: progress-indeterminate 1.2s ease-in-out infinite;
}

@keyframes progress-indeterminate {
  0% {
    transform: translateX(-100%);
  }
  100% {
    transform: translateX(250%);
  }
}
</style>
