<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import Modal from "./Modal.svelte";
  import { t } from "$lib/i18n";
  import { updateState, checkForUpdates } from "$lib/updates.svelte";

  const DOWNLOAD_ICON = `<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M12 3v12m0 0-4-4m4 4 4-4M5 21h14" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`;

  const RELEASES_URL = "https://github.com/simotro/rclone-easy/releases/latest";

  let open = $state(false);
  let installing = $state(false);
  let installError = $state<string | null>(null);

  // Un solo controllo automatico all'avvio dell'app (non ripetuto ad ogni
  // montaggio del componente, che con questa disposizione resta comunque
  // montato una sola volta) — un controllo manuale resta disponibile dalle
  // impostazioni (SettingsButton), che riusa lo stesso stato condiviso.
  $effect(() => {
    checkForUpdates();
  });

  async function installNow() {
    const s = updateState();
    if (s.status !== "available") return;
    installing = true;
    installError = null;
    try {
      await s.update.downloadAndInstall();
      // L'AppImage/l'installer NSIS sono già stati sostituiti sul disco a
      // questo punto — restart_app (non il semplice relaunch del plugin
      // updater) fa ripartire l'app passando comunque dal nostro
      // spegnimento pulito del demone rclone rcd, vedi lib.rs::restart_app.
      await invoke("restart_app");
    } catch (error) {
      installError = String(error);
      installing = false;
    }
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
        <div class="actions">
          {#if s.installKind === "appimage" || s.installKind === "windows"}
            <button type="button" onclick={installNow} disabled={installing}>
              {installing ? $t("update.installing") : $t("update.installNow")}
            </button>
          {:else}
            <button type="button" onclick={openDownloadPage}>{$t("update.openDownloadPage")}</button>
          {/if}
        </div>
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
  justify-content: flex-end;
}
</style>
