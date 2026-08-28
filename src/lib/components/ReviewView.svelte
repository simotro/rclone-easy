<script lang="ts">
  // Contenuto di .rclone-easy-duplicates-review (duplicates.rs) — i
  // duplicati messi da parte con "Sposta per revisione" invece di eliminati
  // subito. Stesso schema di TrashView.svelte, con due differenze: le voci
  // vengono da entrambi i lati del job insieme (un solo comando RC, non uno
  // per lato) e l'azione qui è "elimina definitivamente", non "ripristina"
  // (ripristinare ricreerebbe il duplicato che l'utente aveva già deciso di
  // mettere da parte).
  import { invoke } from "@tauri-apps/api/core";
  import { t } from "$lib/i18n";
  import type { ReviewEntry } from "$lib/types";

  let {
    remoteName,
    localRoot,
    path1,
    path2,
    onRefresh,
  }: {
    remoteName: string;
    localRoot: string;
    path1: string;
    path2: string;
    onRefresh?: () => void | Promise<void>;
  } = $props();

  let rows = $state<ReviewEntry[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let busyId = $state<string | null>(null);
  let pendingDeleteId = $state<string | null>(null);

  // Stessa gating di DuplicateGroupModal: solo Drive ha una sintassi di
  // apertura per ID nota.
  let remoteKind = $state<string | null>(null);
  let canOpenInBrowser = $derived(remoteKind === "drive");

  async function load() {
    loading = true;
    error = null;
    try {
      const [kindResult, rowsResult] = await Promise.all([
        invoke<{ kind: string }>("get_remote_for_edit", { name: remoteName }).then((r) => r.kind),
        invoke<ReviewEntry[]>("list_review_entries", { path1, path2 }),
      ]);
      remoteKind = kindResult;
      rows = rowsResult;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  load();

  async function openLocal(row: ReviewEntry) {
    await invoke("open_local_duplicate", { localRoot, name: row.name });
  }

  async function openRemote(row: ReviewEntry) {
    await invoke("open_url_in_browser", { url: `https://drive.google.com/open?id=${encodeURIComponent(row.id)}` });
  }

  async function confirmDelete(row: ReviewEntry) {
    busyId = row.id;
    error = null;
    try {
      await invoke("delete_review_entry", { fs: row.fs, reviewPath: row.reviewPath });
      rows = rows.filter((r) => r.id !== row.id);
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

<div class="review-view">
  {#if loading}
    <p>{$t("review.loading")}</p>
  {:else if error}
    <p class="error">✗ {error}</p>
  {:else if rows.length === 0}
    <p class="hint">{$t("review.empty")}</p>
  {:else}
    <ul class="entries">
      {#each rows as row (row.id)}
        <li class="entry">
          <div class="entry-info">
            <span class="entry-path">{row.name}</span>
            <span class="entry-meta">
              {$t("review.movedOn", { values: { when: formatWhen(row.modTime) } })} · {formatSize(row.size)}
            </span>
          </div>
          {#if pendingDeleteId === row.id}
            <div class="inline-warning">
              <p>{$t("review.deleteConfirm")}</p>
              <div class="row-actions modal-actions">
                <button type="button" onclick={() => (pendingDeleteId = null)} disabled={busyId === row.id}>{$t("common.cancel")}</button>
                <button type="button" class="btn-danger" onclick={() => confirmDelete(row)} disabled={busyId === row.id}>
                  {busyId === row.id ? $t("review.deleting") : $t("common.confirm")}
                </button>
              </div>
            </div>
          {:else}
            <div class="entry-actions">
              <button type="button" onclick={() => openLocal(row)} disabled={busyId !== null}>{$t("duplicates.openLocal")}</button>
              {#if canOpenInBrowser}
                <button type="button" onclick={() => openRemote(row)} disabled={busyId !== null}>{$t("duplicates.openRemote")}</button>
              {/if}
              <button type="button" class="btn-danger" onclick={() => (pendingDeleteId = row.id)} disabled={busyId !== null}>
                {$t("review.deletePermanently")}
              </button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
.review-view {
  display: flex;
  flex-direction: column;
  gap: 0.6em;
}

.entries {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.5em;
  max-height: 50vh;
  overflow-y: auto;
}

.entry {
  display: flex;
  flex-direction: column;
  gap: 0.5em;
  padding: 0.6em 0.8em;
  border-radius: 8px;
  background-color: var(--surface-tint);
}

.entry-info {
  display: flex;
  flex-direction: column;
  gap: 0.15em;
  min-width: 0;
}

.entry-path {
  font-weight: 600;
  word-break: break-word;
}

.entry-meta {
  font-size: 0.82em;
  opacity: 0.75;
  word-break: break-word;
}

.entry-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4em;
}
</style>
