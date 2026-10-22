<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { t } from "$lib/i18n";
  import type { ActivityEvent, ActivityPage, BisyncJob, SyncJob } from "$lib/types";

  const PAGE_SIZE = 200;
  const KIND_FILTERS = ["all", "warnings", "errors", "copied", "deleted", "moved", "mounts"] as const;

  let remoteFilter = $state("");
  let kindFilter = $state<(typeof KIND_FILTERS)[number]>("all");
  let search = $state("");
  let debouncedSearch = $state("");

  let events = $state<ActivityEvent[]>([]);
  let total = $state(0);
  let remotes = $state<string[]>([]);
  let loadError = $state<string | null>(null);
  let loadingMore = $state(false);

  let bisyncJobs = $state<BisyncJob[]>([]);
  let backupJobs = $state<SyncJob[]>([]);
  let confirmingClear = $state(false);

  function filterArgs(offset: number) {
    return {
      filter: {
        remote: remoteFilter || null,
        kind: kindFilter,
        search: debouncedSearch || null,
        offset,
        limit: PAGE_SIZE,
      },
    };
  }

  // Ricarica dall'inizio: usata al cambio di filtro e dal polling. Il
  // polling non tocca l'elenco se l'utente ha già caricato più pagine, per
  // non fargli perdere la posizione.
  async function reload(force: boolean) {
    if (!force && events.length > PAGE_SIZE) return;
    try {
      const page = await invoke<ActivityPage>("list_activity", filterArgs(0));
      events = page.events;
      total = page.total;
      remotes = page.remotes;
      loadError = null;
    } catch (error) {
      loadError = String(error);
    }
  }

  async function loadMore() {
    loadingMore = true;
    try {
      const page = await invoke<ActivityPage>("list_activity", filterArgs(events.length));
      events = [...events, ...page.events];
      total = page.total;
    } catch (error) {
      loadError = String(error);
    } finally {
      loadingMore = false;
    }
  }

  async function loadRunning() {
    try {
      const [b, j] = await Promise.all([invoke<BisyncJob[]>("list_bisync_jobs"), invoke<SyncJob[]>("list_jobs")]);
      bisyncJobs = b;
      backupJobs = j;
    } catch {
      // Silenzioso: la sezione "In corso" resta com'era fino al prossimo giro.
    }
  }

  async function clearLog() {
    if (!confirmingClear) {
      confirmingClear = true;
      return;
    }
    confirmingClear = false;
    await invoke("clear_activity");
    await reload(true);
  }

  $effect(() => {
    const value = search;
    const timer = setTimeout(() => (debouncedSearch = value), 300);
    return () => clearTimeout(timer);
  });

  // Cambiando un filtro l'elenco riparte dall'inizio.
  $effect(() => {
    remoteFilter;
    kindFilter;
    debouncedSearch;
    confirmingClear = false;
    reload(true);
  });

  $effect(() => {
    loadRunning();
    const slow = setInterval(() => reload(false), 5000);
    const fast = setInterval(loadRunning, 2000);
    return () => {
      clearInterval(slow);
      clearInterval(fast);
    };
  });

  let runningBisync = $derived(bisyncJobs.filter((j) => j.isRunning));
  let runningBackups = $derived(backupJobs.filter((j) => j.isRunning));
  let nothingRunning = $derived(runningBisync.length === 0 && runningBackups.length === 0);

  function formatSize(bytes: number): string {
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

  function formatWhen(whenUnix: number): string {
    return new Date(whenUnix * 1000).toLocaleString(undefined, {
      day: "2-digit",
      month: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  }

  function arrow(direction: string): string {
    return direction === "upload" ? "↑" : direction === "download" ? "↓" : direction === "sync" ? "⇅" : "";
  }

  const KIND_SYMBOL: Record<string, string> = {
    copied: "✓",
    deleted: "✕",
    moved: "➜",
    conflict: "⚠",
    warning: "⚠",
    error: "✗",
    mounted: "●",
    unmounted: "○",
  };
</script>

<main class="container">
  <a href="/" class="back-link">← {$t("activity.back")}</a>
  <h1>{$t("activity.title")}</h1>

  <section class="running">
    <h2>{$t("activity.running")}</h2>
    {#if nothingRunning}
      <p class="muted">{$t("activity.nothingRunning")}</p>
    {:else}
      <ul class="running-list">
        {#each runningBisync as job (job.name)}
          <li>
            <strong>{job.name}</strong>
            <span class="muted">
              {job.progress ? $t(`activity.phase.${job.progress.phase}`) : $t("activity.phase.preparing")}
              {#if job.progress && job.progress.queuedTotal > 0}
                · {$t("activity.queued", { values: { count: job.progress.queuedTotal } })}
              {/if}
            </span>
            {#each job.progress?.transferring ?? [] as file (file.name)}
              <div class="transfer-line" title={file.name}>
                {arrow(file.direction)} {file.name}
                <span class="muted">
                  {formatSize(file.bytes)} / {formatSize(file.size)}{file.size > 0 ? ` (${Math.min(100, Math.floor((file.bytes / file.size) * 100))}%)` : ""}
                </span>
              </div>
            {/each}
          </li>
        {/each}
        {#each runningBackups as job (job.name)}
          <li>
            <strong>{job.name}</strong>
            <span class="muted">{$t("activity.backupRunning")}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="filters">
    <label>
      <span class="muted">{$t("activity.filterRemote")}</span>
      <select bind:value={remoteFilter}>
        <option value="">{$t("activity.allRemotes")}</option>
        {#each remotes as name (name)}
          <option value={name}>{name}</option>
        {/each}
      </select>
    </label>
    <label>
      <span class="muted">{$t("activity.filterKind")}</span>
      <select bind:value={kindFilter}>
        {#each KIND_FILTERS as group (group)}
          <option value={group}>{$t(`activity.kinds.${group}`)}</option>
        {/each}
      </select>
    </label>
    <label class="search">
      <span class="muted">&nbsp;</span>
      <input type="search" bind:value={search} placeholder={$t("activity.searchPlaceholder")} />
    </label>
  </section>

  {#if loadError}
    <p class="error">✗ {loadError}</p>
  {/if}

  {#if events.length === 0}
    <p class="muted empty">{$t("activity.empty")}</p>
  {:else}
    <ul class="event-list">
      {#each events as event, i (i)}
        <li class="event {event.kind}">
          <span class="symbol" title={$t(`activity.kind.${event.kind}`)}>{KIND_SYMBOL[event.kind] ?? "•"}</span>
          <div class="event-body">
            <div class="event-main">
              <span class="event-path" title={event.path || event.message}>
                {#if arrow(event.direction)}<span class="direction">{arrow(event.direction)}</span>{/if}
                {event.path || $t(`activity.kind.${event.kind}`)}
              </span>
              {#if event.size !== null}<span class="muted size">{formatSize(event.size)}</span>{/if}
            </div>
            <div class="event-meta muted">
              {formatWhen(event.whenUnix)} · {event.remote}{event.job && event.job !== event.remote ? ` · ${event.job}` : ""}
              {#if event.path} · {$t(`activity.kind.${event.kind}`)}{/if}
            </div>
            {#if event.message && (event.kind === "error" || event.kind === "warning" || event.kind === "conflict" || event.kind === "unmounted" || event.kind === "mounted")}
              <div class="event-message">{event.message}</div>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
    <div class="list-footer">
      <span class="muted">{$t("activity.showing", { values: { shown: events.length, total } })}</span>
      {#if events.length < total}
        <button type="button" onclick={loadMore} disabled={loadingMore}>{$t("activity.loadMore")}</button>
      {/if}
    </div>
  {/if}

  <footer class="page-footer">
    <span class="muted">{$t("activity.retention")}</span>
    <button type="button" class="clear-button" onclick={clearLog}>
      {confirmingClear ? $t("activity.clearConfirm") : $t("activity.clear")}
    </button>
  </footer>
</main>

<style>
.container {
  margin: 0 auto;
  max-width: 44em;
  padding: 4vh 1.5em;
}

.back-link {
  display: inline-block;
  margin-bottom: 1em;
}

h2 {
  font-size: 1em;
  margin: 0 0 0.4em;
}

.muted {
  color: var(--text-muted);
  font-size: 0.85em;
}

.running {
  margin-bottom: 1.2em;
  padding: 0.8em 1em;
  border-radius: 10px;
  background-color: var(--bg-surface);
  box-shadow: var(--shadow-md);
}

.running-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.6em;
}

.transfer-line {
  font-size: 0.8em;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.filters {
  display: flex;
  flex-wrap: wrap;
  gap: 0.8em;
  align-items: flex-end;
  margin-bottom: 1em;
}

.filters label {
  display: flex;
  flex-direction: column;
  gap: 0.2em;
}

.filters .search {
  flex: 1;
  min-width: 10em;
}

.event-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
}

.event {
  display: flex;
  gap: 0.7em;
  padding: 0.55em 0.2em;
  border-bottom: 1px solid var(--border-color-subtle);
}

.symbol {
  flex-shrink: 0;
  width: 1.4em;
  text-align: center;
  font-weight: 700;
}

.event.copied .symbol {
  color: var(--status-green, #3fa45b);
}

.event.deleted .symbol,
.event.moved .symbol {
  color: var(--warning-text);
}

.event.warning .symbol,
.event.conflict .symbol {
  color: var(--warning-text);
}

.event.error .symbol {
  color: var(--error);
}

.event-body {
  min-width: 0;
  flex: 1;
}

.event-main {
  display: flex;
  justify-content: space-between;
  gap: 1em;
}

.event-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.direction {
  color: var(--accent);
  margin-right: 0.2em;
}

.size {
  flex-shrink: 0;
}

.event-meta {
  font-size: 0.75em;
}

.event-message {
  margin-top: 0.15em;
  font-size: 0.8em;
  color: var(--text-muted);
  overflow-wrap: anywhere;
}

.event.error .event-message {
  color: var(--error);
}

.list-footer,
.page-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 1em;
  margin-top: 1em;
}

.page-footer {
  margin-top: 2em;
}

.empty {
  margin: 2em 0;
  text-align: center;
}
</style>
