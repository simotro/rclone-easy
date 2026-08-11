<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import Modal from "./Modal.svelte";
  import { t } from "$lib/i18n";

  let {
    open = $bindable(false),
    remoteName,
    onSelect,
  }: {
    open: boolean;
    remoteName: string;
    onSelect: (path: string) => void;
  } = $props();

  type Entry = { name: string; path: string };

  let entries = $state<Entry[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // Solo il primo livello sotto la radice, niente navigazione più in
  // profondità: scendere di livello in livello significava una chiamata RC
  // per ogni click, lenta su remote reali — un'unica chiamata alla radice
  // ogni apertura è molto più veloce e resta comunque utile per il caso
  // comune (scegliere una cartella di primo livello).
  $effect(() => {
    if (open) load();
  });

  async function load() {
    loading = true;
    error = null;
    try {
      entries = await invoke<Entry[]>("list_remote_dir", { remote: remoteName, path: "" });
      entries.sort((a, b) => a.name.localeCompare(b.name));
    } catch (e) {
      error = String(e);
      entries = [];
    } finally {
      loading = false;
    }
  }

  function select(path: string) {
    onSelect(path);
    open = false;
  }
</script>

<Modal bind:open title={$t("folderPicker.title", { values: { remote: remoteName } })}>
  <div class="picker">
    {#if loading}
      <p class="hint">{$t("common.loading")}</p>
    {:else if error}
      <p class="error">✗ {error}</p>
    {/if}

    <ul class="entries">
      <li>
        <button type="button" class="root-entry" onclick={() => select("")}>{$t("folderPicker.useRoot")}</button>
      </li>
      {#each entries as entry (entry.path)}
        <li>
          <button type="button" onclick={() => select(entry.path)}>📁 {entry.name}</button>
        </li>
      {/each}
    </ul>
  </div>
</Modal>

<style>
.picker {
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
  gap: 0.2em;
  max-height: 50vh;
  overflow-y: auto;
}

.entries button {
  width: 100%;
  text-align: left;
  background: none;
  border: none;
  box-shadow: none;
  padding: 0.5em 0.6em;
  border-radius: 6px;
  color: var(--text-color);
  font-weight: 400;
}

.entries button:hover {
  background-color: var(--surface-tint);
}

.root-entry {
  font-weight: 600;
  border-bottom: 1px solid var(--border-color-subtle);
  border-radius: 0;
  margin-bottom: 0.3em;
  padding-bottom: 0.6em;
}

.hint {
  font-size: 0.9em;
  color: var(--text-muted);
}
</style>
