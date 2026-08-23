<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { goto } from "$app/navigation";
  import RemoteRow from "$lib/components/RemoteRow.svelte";
  import SettingsButton from "$lib/components/SettingsButton.svelte";
  import type { MountEntry, SyncJob, BisyncJob } from "$lib/types";
  import { t } from "$lib/i18n";

  type CheckState<T> = { status: "loading" } | { status: "ok"; value: T } | { status: "error"; message: string };

  let ownRemotes = $state<CheckState<string[]>>({ status: "loading" });
  // Nessuna sezione diagnostica dedicata: qui basta sapere se qualcosa di
  // fondamentale non funziona (rclone non trovato, demone rcd non
  // partito) — senza uno dei due l'app è sostanzialmente inutilizzabile,
  // quindi un avviso silenzioso-se-tutto-ok è sufficiente, niente da
  // esplorare in un pannello a parte.
  let startupWarning = $state<string | null>(null);

  async function runCheck<T>(command: string): Promise<CheckState<T>> {
    try {
      const value = await invoke<T>(command);
      return { status: "ok", value };
    } catch (error) {
      return { status: "error", message: String(error) };
    }
  }

  async function loadOwnRemotes() {
    ownRemotes = await runCheck<string[]>("list_own_remotes");
  }

  // Mount/backup/bisync caricati UNA volta qui, condivisi da tutte le righe
  // (RemoteRow non fa più fetch proprio): con N remote, invoke tre liste
  // complete una volta sola invece di farlo N volte con lo stesso risultato
  // — indispensabile per poter aggiornare periodicamente (serve per il
  // countdown "prossima esecuzione") senza far scalare il costo con il
  // numero di remote.
  let mounts = $state<MountEntry[]>([]);
  let jobs = $state<SyncJob[]>([]);
  let bisyncJobs = $state<BisyncJob[]>([]);

  async function loadServices() {
    try {
      const [m, j, b] = await Promise.all([
        invoke<MountEntry[]>("list_mounts"),
        invoke<SyncJob[]>("list_jobs"),
        invoke<BisyncJob[]>("list_bisync_jobs"),
      ]);
      mounts = m;
      jobs = j;
      bisyncJobs = b;
    } catch {
      // Silenzioso: ogni RemoteRow mostra comunque lo stato "nessuna
      // operazione" se le liste restano vuote, non serve un banner
      // separato per un fallimento di un aggiornamento periodico.
    }
  }

  async function onRemotesChanged() {
    await Promise.all([loadOwnRemotes(), loadServices()]);
  }

  $effect(() => {
    invoke<string>("check_rclone_installed").catch((error) => {
      startupWarning = $t("home.rcloneNotAvailable", { values: { error } });
    });
    invoke<string>("rcd_status").catch((error) => {
      startupWarning = $t("home.rcdNotResponding", { values: { error } });
    });
    loadOwnRemotes();
    loadServices();
    // 10s: abbastanza spesso da tenere il countdown della prossima
    // esecuzione automatica ragionevolmente aggiornato, abbastanza raro da
    // non pesare (tre chiamate IPC leggere ogni 10s, indipendentemente da
    // quanti remote ci sono).
    const id = setInterval(loadServices, 10000);
    return () => clearInterval(id);
  });
</script>

<main class="container">
  <div class="app-title">
    <img src="/app-icon.png" alt="" class="app-icon" />
    <h1>Rclone Easy</h1>
  </div>

  {#if startupWarning}
    <p class="startup-warning">⚠ {startupWarning}</p>
  {/if}

  <section class="remotes-section">
    <div class="top-actions">
      <button type="button" class="add-remote-button" onclick={() => goto("/nuovo-remote")}>
        {$t("home.addRemote")}
      </button>
      <SettingsButton {onRemotesChanged} />
    </div>

    {#if ownRemotes.status === "loading"}
      <p>{$t("home.checking")}</p>
    {:else if ownRemotes.status === "error"}
      <p class="error">✗ {ownRemotes.message}</p>
    {:else if ownRemotes.value.length === 0}
      <p class="empty">{$t("home.noRemotesYet")}</p>
    {:else}
      <!-- Scorre qui, non l'intera finestra (audit UX 21/8/2026, punto E):
           con molti remote configurati, prima l'intero corpo della pagina
           scorreva via, portandosi dietro anche "Aggiungi Remote" e
           "Impostazioni". `min-height: 0` è necessario perché un figlio
           flex non si restringe mai sotto la dimensione del suo contenuto
           per conto suo — senza, `overflow-y: auto` qui sotto non avrebbe
           mai occasione di scattare. -->
      <div class="remote-list-scroll">
        <ul class="remote-list">
          {#each ownRemotes.value as name (name)}
            <RemoteRow remoteName={name} {mounts} {jobs} {bisyncJobs} onDeleted={loadOwnRemotes} onRefresh={loadServices} />
          {/each}
        </ul>
      </div>
    {/if}
  </section>
</main>

<style>
.container {
  margin: 0 auto;
  max-width: 44em;
  padding: 4vh 1.5em;
  height: 100%;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
}

.app-title {
  display: flex;
  align-items: center;
  gap: 0.5em;
}

.app-icon {
  width: 4.4em;
  height: 4.4em;
}

.app-title h1 {
  margin: 0;
}

.remotes-section {
  margin-top: 1.5em;
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
}

.remote-list-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  /* Un po' di margine per non far toccare la scrollbar all'ultima riga. */
  padding-bottom: 0.2em;
}

.empty {
  font-size: 0.9em;
}

.top-actions {
  display: flex;
  gap: 0.6em;
  flex-wrap: wrap;
  margin-bottom: 1em;
}

.add-remote-button {
  display: inline-flex;
  align-items: center;
  gap: 0.4em;
}

.remote-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 0.6em;
}

.startup-warning {
  margin-top: 1em;
  padding: 0.7em 1em;
  border-radius: 8px;
  background-color: var(--warning-bg);
  color: var(--warning-text);
  font-size: 0.9em;
}
</style>
