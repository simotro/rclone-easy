<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open as openFileDialog } from "@tauri-apps/plugin-dialog";

  // Usato sia dalla pagina dedicata `/importa-remote` (raggiunta dal wizard
  // "Aggiungi remote") sia incorporato nel modal "Importa / Esporta" della
  // home, così la logica di rilevamento/selezione/import non è duplicata
  // in due posti — vedi +page.svelte per l'uso incorporato.
  let { onImported }: { onImported?: () => void } = $props();

  type ExistingRemote = { name: string; kind: string };
  type ItemResult = { status: "importing" | "done" | "error"; message?: string };

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let existing = $state<ExistingRemote[]>([]);
  let ownNames = $state<string[]>([]);
  let selected = $state<Set<string>>(new Set());
  let results = $state<Record<string, ItemResult>>({});
  let importing = $state(false);
  // `null` = config di sistema (comportamento di rclone di default). Un
  // percorso esplicito arriva solo se l'utente sceglie un file diverso
  // (es. un rclone.conf portato da un'altra macchina).
  let configPath = $state<string | null>(null);
  let standardConfigPath = $state<string | null>(null);

  async function loadExisting() {
    loading = true;
    loadError = null;
    try {
      const [existingRemotes, own] = await Promise.all([
        invoke<ExistingRemote[]>("detect_existing_remotes", { configPath }),
        invoke<string[]>("list_own_remotes"),
      ]);
      existing = existingRemotes;
      ownNames = own;
      selected = new Set();
      results = {};
    } catch (error) {
      loadError = String(error);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    invoke<string | null>("standard_rclone_config_path")
      .then((path) => (standardConfigPath = path))
      .catch(() => {});
    loadExisting();
  });

  async function chooseDifferentConfigFile() {
    const selectedFile = await openFileDialog({
      title: "Scegli un file di configurazione rclone",
      multiple: false,
      defaultPath: standardConfigPath ?? undefined,
    });
    if (typeof selectedFile !== "string") return;
    configPath = selectedFile;
    await loadExisting();
  }

  function useSystemConfig() {
    configPath = null;
    loadExisting();
  }

  function isConflicting(name: string): boolean {
    return ownNames.includes(name);
  }

  function toggle(name: string) {
    const next = new Set(selected);
    if (next.has(name)) next.delete(name);
    else next.add(name);
    selected = next;
  }

  let selectableNames = $derived(existing.filter((r) => !isConflicting(r.name)).map((r) => r.name));
  let allSelected = $derived(selectableNames.length > 0 && selectableNames.every((n) => selected.has(n)));

  function toggleAll() {
    selected = allSelected ? new Set() : new Set(selectableNames);
  }

  async function importSelected() {
    importing = true;
    for (const name of selected) {
      results = { ...results, [name]: { status: "importing" } };
      try {
        await invoke("import_remote", { name, configPath });
        results = { ...results, [name]: { status: "done" } };
        onImported?.();
      } catch (error) {
        results = { ...results, [name]: { status: "error", message: String(error) } };
      }
    }
    importing = false;
  }
</script>

<div class="import-remotes">
  <p class="subtitle">
    Remote già presenti in un <code>rclone.conf</code>: scegli quali portare nella config di Rclone Easy. Vengono
    comunque verificati prima di essere salvati, come per un nuovo remote.
  </p>

  <div class="config-source">
    {#if configPath}
      <span class="hint">Sorgente: <code>{configPath}</code></span>
      <button type="button" class="link-button" onclick={useSystemConfig} disabled={loading || importing}>
        Usa il config di sistema
      </button>
    {:else}
      <span class="hint">Sorgente: config di sistema</span>
    {/if}
    <button type="button" class="link-button" onclick={chooseDifferentConfigFile} disabled={loading || importing}>
      Scegli un altro file di configurazione…
    </button>
  </div>

  {#if loading}
    <p>Verifica in corso…</p>
  {:else if loadError}
    <p class="error">✗ {loadError}</p>
  {:else if existing.length === 0}
    <p>Nessun remote trovato in questo <code>rclone.conf</code>.</p>
  {:else}
    <label class="select-all">
      <input type="checkbox" checked={allSelected} onchange={toggleAll} disabled={selectableNames.length === 0} />
      Seleziona tutti
    </label>

    <ul class="remote-list">
      {#each existing as remote (remote.name)}
        {@const conflict = isConflicting(remote.name)}
        {@const result = results[remote.name]}
        <li>
          <label class:disabled={conflict}>
            <input
              type="checkbox"
              checked={selected.has(remote.name)}
              disabled={conflict || importing}
              onchange={() => toggle(remote.name)}
            />
            {remote.name} <span class="kind">({remote.kind})</span>
          </label>
          {#if conflict}
            <span class="hint">già presente in Rclone Easy</span>
          {:else if result?.status === "importing"}
            <span class="hint">verifica in corso…</span>
          {:else if result?.status === "done"}
            <span class="ok">✓ importato</span>
          {:else if result?.status === "error"}
            <span class="error">✗ {result.message}</span>
          {/if}
        </li>
      {/each}
    </ul>

    <button onclick={importSelected} disabled={selected.size === 0 || importing}>
      {importing ? "Importazione in corso…" : "Importa selezionati"}
    </button>
  {/if}
</div>

<style>
.import-remotes {
  display: flex;
  flex-direction: column;
  gap: 0.5em;
}

.subtitle {
  color: var(--text-muted);
  margin: 0;
}

.config-source {
  display: flex;
  align-items: center;
  gap: 1em;
  flex-wrap: wrap;
  margin-top: 0.5em;
}

.link-button {
  background: none;
  border: none;
  box-shadow: none;
  padding: 0;
  color: var(--accent);
  cursor: pointer;
  font-size: 0.9em;
}

.link-button:disabled {
  cursor: default;
  opacity: 0.6;
}

code {
  font-family: "Cascadia Code", "Fira Code", monospace;
  background-color: var(--code-bg);
  padding: 0.1em 0.4em;
  border-radius: 4px;
}

.select-all {
  display: block;
  margin-top: 1em;
  font-weight: 500;
}

.remote-list {
  list-style: none;
  padding: 0;
  margin-top: 0.75em;
  display: flex;
  flex-direction: column;
  gap: 0.5em;
}

.remote-list li {
  display: flex;
  align-items: center;
  gap: 0.75em;
  padding: 0.5em 0.7em;
  border-radius: 6px;
  background-color: var(--bg-surface);
  box-shadow: var(--shadow-sm);
}

label.disabled {
  opacity: 0.55;
}

.kind {
  color: var(--text-muted);
}

.hint {
  color: var(--text-muted);
  font-size: 0.9em;
}

.import-remotes > button {
  margin-top: 1em;
  align-self: flex-start;
}
</style>
