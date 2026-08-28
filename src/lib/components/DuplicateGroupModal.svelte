<script lang="ts">
  // Dettaglio di un gruppo di nomi duplicati (bisync.rs::extract_duplicate_names)
  // con azioni per singolo oggetto — pensato per Google Drive (l'unico
  // backend comune dove un nome duplicato può esistere per costruzione),
  // ma non presume nulla sul tipo di remote oltre a quello: "Apri" compare
  // solo se `remoteKind` lo supporta (vedi il commento lì sotto).
  import { invoke } from "@tauri-apps/api/core";
  import { untrack } from "svelte";
  import Modal from "./Modal.svelte";
  import { t } from "$lib/i18n";
  import type { DuplicateObject } from "$lib/types";

  let {
    open = $bindable(false),
    remoteName,
    path1,
    path2,
    name,
    onRefresh,
  }: {
    open: boolean;
    remoteName: string;
    path1: string;
    path2: string;
    name: string;
    onRefresh?: () => void | Promise<void>;
  } = $props();

  let objects = $state<DuplicateObject[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let busyId = $state<string | null>(null);
  let pendingDeleteId = $state<string | null>(null);

  // Solo Drive ha una sintassi di apertura per ID nota — "Apri" resta
  // nascosto per qualunque altro tipo di remote invece di mostrare un link
  // che non porterebbe da nessuna parte.
  let remoteKind = $state<string | null>(null);
  let canOpenInBrowser = $derived(remoteKind === "drive");

  async function load(currentRemoteName: string, currentPath1: string, currentPath2: string, currentName: string) {
    loading = true;
    error = null;
    try {
      const [kindResult, objectsResult] = await Promise.all([
        invoke<{ kind: string }>("get_remote_for_edit", { name: currentRemoteName }).then((r) => r.kind),
        invoke<DuplicateObject[]>("list_duplicate_group", { path1: currentPath1, path2: currentPath2, name: currentName }),
      ]);
      remoteKind = kindResult;
      objects = objectsResult;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  // `path1`/`path2`/`remoteName` arrivano dal job bisync ricaricato ogni 10s
  // da +page.svelte (countdown della prossima esecuzione automatica) — una
  // NUOVA istanza dell'oggetto job ad ogni giro, pur con lo stesso contenuto.
  // Letti dentro `untrack` così l'effetto non li considera una dipendenza:
  // senza questo, il modal ripartiva da capo (spinner, richieste rcd) ogni
  // 10s anche restando fermo sullo stesso gruppo, per un cambiamento che non
  // ha alcun effetto reale sui dati mostrati. Resta invece tracciato `name`,
  // che DEVE far ricaricare quando si passa a un altro nome duplicato senza
  // chiudere il modal (vedi `examineDuplicate` in RemotePanel.svelte).
  $effect(() => {
    if (!open) return;
    const currentName = name;
    untrack(() => load(remoteName, path1, path2, currentName));
  });

  async function openInBrowser(obj: DuplicateObject) {
    await invoke("open_url_in_browser", { url: `https://drive.google.com/open?id=${encodeURIComponent(obj.id)}` });
  }

  async function moveForReview(obj: DuplicateObject) {
    busyId = obj.id;
    error = null;
    try {
      await invoke("move_duplicate_for_review", { fs: obj.fs, id: obj.id, name });
      objects = objects.filter((o) => o.id !== obj.id);
      await onRefresh?.();
    } catch (e) {
      error = String(e);
    } finally {
      busyId = null;
    }
  }

  async function confirmDelete(obj: DuplicateObject) {
    busyId = obj.id;
    error = null;
    try {
      await invoke("delete_duplicate", { fs: obj.fs, id: obj.id, name });
      objects = objects.filter((o) => o.id !== obj.id);
      pendingDeleteId = null;
      await onRefresh?.();
    } catch (e) {
      error = String(e);
    } finally {
      busyId = null;
    }
  }

  function formatWhen(iso: string): string {
    const parsed = new Date(iso);
    return Number.isNaN(parsed.getTime()) ? iso : parsed.toLocaleString();
  }

  function formatSize(bytes: number): string {
    if (bytes < 0) return "?";
    if (bytes < 1024) return `${bytes} B`;
    const units = ["KiB", "MiB", "GiB", "TiB"];
    let value = bytes / 1024;
    let unitIndex = 0;
    while (value >= 1024 && unitIndex < units.length - 1) {
      value /= 1024;
      unitIndex += 1;
    }
    return `${value.toFixed(1)} ${units[unitIndex]}`;
  }
</script>

<Modal bind:open title={$t("duplicates.title", { values: { name } })} elevated>
  {#if loading}
    <p>{$t("common.loading")}</p>
  {:else if error}
    <p class="error">✗ {error}</p>
  {:else if objects.length === 0}
    <p class="hint">{$t("duplicates.resolved")}</p>
  {:else}
    <ul class="objects">
      {#each objects as obj (obj.id)}
        <li class="object-row">
          <div class="object-info">
            <span class="object-meta">{formatSize(obj.size)} · {formatWhen(obj.modTime)}</span>
            <span class="object-id">{obj.id}</span>
          </div>
          {#if pendingDeleteId === obj.id}
            <div class="inline-warning">
              <p>{$t("duplicates.deleteConfirm")}</p>
              <div class="row-actions modal-actions">
                <button type="button" onclick={() => (pendingDeleteId = null)} disabled={busyId === obj.id}>{$t("common.cancel")}</button>
                <button type="button" class="btn-danger" onclick={() => confirmDelete(obj)} disabled={busyId === obj.id}>
                  {busyId === obj.id ? $t("remoteRow.deleting") : $t("common.confirm")}
                </button>
              </div>
            </div>
          {:else}
            <div class="object-actions">
              {#if canOpenInBrowser}
                <button type="button" onclick={() => openInBrowser(obj)} disabled={busyId !== null}>{$t("duplicates.open")}</button>
              {/if}
              <button type="button" onclick={() => moveForReview(obj)} disabled={busyId !== null}>
                {busyId === obj.id ? $t("common.inProgress") : $t("duplicates.moveForReview")}
              </button>
              <button type="button" class="btn-danger" onclick={() => (pendingDeleteId = obj.id)} disabled={busyId !== null}>
                {$t("duplicates.delete")}
              </button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</Modal>

<style>
.objects {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.6em;
}

.object-row {
  display: flex;
  flex-direction: column;
  gap: 0.5em;
  padding: 0.6em 0.8em;
  border-radius: 8px;
  background-color: var(--surface-tint);
}

.object-info {
  display: flex;
  flex-direction: column;
  gap: 0.1em;
  min-width: 0;
}

.object-meta {
  font-size: 0.9em;
  font-weight: 600;
}

.object-id {
  font-size: 0.78em;
  color: var(--text-muted);
  word-break: break-all;
}

.object-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4em;
}
</style>
