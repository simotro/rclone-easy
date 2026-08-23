<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { page } from "$app/stores";
  import RemotePanel from "$lib/components/RemotePanel.svelte";
  import type { MountEntry, SyncJob, BisyncJob } from "$lib/types";
  import { t } from "$lib/i18n";

  const remoteName = $derived(decodeURIComponent($page.params.name ?? ""));
  let prefix = $derived(`${remoteName}:`);

  type RemoteTab = "configura" | "cronologia" | "cestino";

  // Solo tab valide da ?tab= (link "Configura"/voci di avviso dalla tray,
  // vedi +layout.svelte) — qualunque altro valore ricade sul default del
  // pannello (Configura).
  let initialTab = $derived.by<RemoteTab | undefined>(() => {
    const raw = $page.url.searchParams.get("tab");
    if (raw === "cronologia" || raw === "cestino" || raw === "configura") return raw;
    return undefined;
  });

  let mounts = $state<MountEntry[]>([]);
  let jobs = $state<SyncJob[]>([]);
  let bisyncJobs = $state<BisyncJob[]>([]);
  let loaded = $state(false);

  let mountEntry = $derived(mounts.find((m) => m.remote.startsWith(prefix)) ?? null);
  let syncJob = $derived(jobs.find((j) => j.source.startsWith(prefix) || j.destination.startsWith(prefix)) ?? null);
  let bisyncJob = $derived(bisyncJobs.find((j) => j.path1.startsWith(prefix) || j.path2.startsWith(prefix)) ?? null);

  // Stesse tre liste complete della home (nessun comando "solo per questo
  // remote" lato backend), filtrate qui sopra — stesso schema di
  // RemoteRow.svelte, ma questa pagina è indipendente dalla home (si può
  // raggiungere direttamente, es. da un avviso della tray).
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
    } finally {
      loaded = true;
    }
  }

  $effect(() => {
    loadServices();
    // Stesso intervallo della home, per lo stesso motivo (countdown della
    // prossima esecuzione automatica ragionevolmente aggiornato).
    const id = setInterval(loadServices, 10000);
    return () => clearInterval(id);
  });
</script>

<main class="container">
  <a href="/" class="back-link">← {$t("newRemote.backToHome")}</a>
  <h1 class="page-title">{$t("remotePanel.title", { values: { remote: remoteName } })}</h1>

  {#if loaded}
    {#key remoteName}
      <RemotePanel {remoteName} {mountEntry} {syncJob} {bisyncJob} {initialTab} onRefresh={loadServices} />
    {/key}
  {:else}
    <p>{$t("common.loading")}</p>
  {/if}
</main>

<style>
.container {
  margin: 0 auto;
  max-width: 40em;
  padding: 4vh 1.5em;
}

.back-link {
  display: inline-block;
  color: var(--accent);
}

.page-title {
  margin-top: 0.3em;
}
</style>
