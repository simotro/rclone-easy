<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import Modal from "./Modal.svelte";
  import Icon from "./Icon.svelte";
  import RemotePanel from "./RemotePanel.svelte";
  import { now, formatCountdown, nextRunAtMs } from "$lib/now";
  import type { MountEntry, SyncJob, BisyncJob } from "$lib/types";
  import { t } from "$lib/i18n";

  let {
    remoteName,
    mounts,
    jobs,
    bisyncJobs,
    onDeleted,
    onRefresh,
  }: {
    remoteName: string;
    mounts: MountEntry[];
    jobs: SyncJob[];
    bisyncJobs: BisyncJob[];
    onDeleted?: () => void;
    onRefresh?: () => void | Promise<void>;
  } = $props();

  let prefix = $derived(`${remoteName}:`);

  // Filtrate dalle liste passate dal genitore (caricate una sola volta e
  // aggiornate periodicamente lì) invece di un fetch proprio — vedi
  // +page.svelte per il perché: con N remote un fetch per riga farebbe N
  // chiamate IPC ripetute per lo stesso risultato ad ogni aggiornamento.
  let mountEntry = $derived(mounts.find((m) => m.remote.startsWith(prefix)) ?? null);
  let syncJob = $derived(jobs.find((j) => j.source.startsWith(prefix) || j.destination.startsWith(prefix)) ?? null);
  let bisyncJob = $derived(bisyncJobs.find((j) => j.path1.startsWith(prefix) || j.path2.startsWith(prefix)) ?? null);

  type ServiceKind = "mount" | "backup" | "bisync";

  let activeService = $derived.by<ServiceKind | null>(() => {
    if (mountEntry?.mounted) return "mount";
    if (syncJob?.autoIntervalMinutes !== null && syncJob !== null) return "backup";
    if (bisyncJob?.autoIntervalMinutes !== null && bisyncJob !== null) return "bisync";
    return null;
  });

  // Il servizio da mostrare sull'indicatore: quello attivo se c'è, altrimenti
  // il primo che ha comunque una configurazione salvata (manuale, mai
  // automatizzata), altrimenti nessuno — un solo indicatore invece delle 3
  // icone mount/backup/bisync sempre visibili (audit UX 21/8/2026, punto B):
  // sono comunque una scelta mutuamente esclusiva, non serve vederle tutte
  // e tre insieme per capire qual è quella in gioco.
  let displayService = $derived.by<ServiceKind | null>(() => {
    if (activeService) return activeService;
    if (mountEntry) return "mount";
    if (syncJob) return "backup";
    if (bisyncJob) return "bisync";
    return null;
  });

  const SERVICE_LABELS: Record<ServiceKind, string> = $derived({
    mount: $t("remoteRow.serviceLabel.mount"),
    backup: $t("remoteRow.serviceLabel.backup"),
    bisync: $t("remoteRow.serviceLabel.bisync"),
  });

  // Stato onesto della riga: guarda l'ULTIMO TENTATIVO del servizio attivo,
  // non solo l'ultimo successo — prima di questa modifica un remote con un
  // errore reale in cronologia poteva mostrare "nessuna operazione
  // eseguita", perché quel testo ignorava del tutto i tentativi falliti.
  // "conflict" è un terzo esito solo per bisync (successo, ma con file da
  // rivedere).
  type AttemptOutcome = "ok" | "conflict" | "failed";
  type LastAttempt = { whenUnix: number; outcome: AttemptOutcome; reason: string | null };

  let lastAttempt = $derived.by<LastAttempt | null>(() => {
    if (activeService === "mount" && mountEntry) {
      const latest = mountEntry.history[0];
      if (!latest) return null;
      return { whenUnix: latest.whenUnix, outcome: latest.success ? "ok" : "failed", reason: latest.success ? null : latest.message };
    }
    if (activeService === "backup" && syncJob) {
      const latest = syncJob.history[0];
      if (!latest) return null;
      return { whenUnix: latest.whenUnix, outcome: latest.success ? "ok" : "failed", reason: latest.success ? null : latest.message };
    }
    if (activeService === "bisync" && bisyncJob) {
      const latest = bisyncJob.history[0];
      if (!latest) return null;
      if (!latest.success) return { whenUnix: latest.whenUnix, outcome: "failed", reason: latest.message };
      if (latest.conflictPaths.length > 0) return { whenUnix: latest.whenUnix, outcome: "conflict", reason: null };
      return { whenUnix: latest.whenUnix, outcome: "ok", reason: null };
    }
    return null;
  });

  function formatWhen(whenUnix: number): string {
    return new Date(whenUnix * 1000).toLocaleString();
  }

  // Estratto breve del motivo di un fallimento per la riga collassata — mai
  // il messaggio grezzo per intero lì (può essere lungo quanto un errore
  // tecnico di rclone/Google, vedi l'audit UX): cambierebbe l'altezza della
  // riga in modo imprevedibile. Il testo completo resta sempre raggiungibile
  // nel pannello del remote.
  function truncate(text: string, max = 70): string {
    const clean = text.trim();
    return clean.length > max ? `${clean.slice(0, max - 1)}…` : clean;
  }

  // Conto alla rovescia verso la prossima esecuzione automatica — solo per
  // backup/bisync (il mount non ha un "prossimo giro", resta montato finché
  // non lo si smonta), e solo per il servizio effettivamente attivo: la
  // mutua esclusività garantisce che al massimo uno tra backup e bisync
  // possa avere l'automazione accesa per questo remote alla volta.
  let countdownText = $derived.by<string | null>(() => {
    if (syncJob?.isDryRunning || bisyncJob?.isDryRunning) return $t("remoteRow.dryRunInProgress");
    if (activeService === "backup" && syncJob) {
      if (syncJob.isRunning) return $t("remoteRow.backupRunning");
      if (syncJob.history.length === 0) return $t("remoteRow.backupFirstSoon");
      const target = nextRunAtMs(syncJob.history[0].whenUnix, syncJob.autoIntervalMinutes);
      return target === null ? null : $t("remoteRow.nextBackup", { values: { when: formatCountdown(target, $now) } });
    }
    if (activeService === "bisync" && bisyncJob) {
      if (bisyncJob.isRunning) return $t("remoteRow.bisyncRunning");
      if (bisyncJob.history.length === 0) return $t("remoteRow.bisyncFirstSoon");
      const target = nextRunAtMs(bisyncJob.history[0].whenUnix, bisyncJob.autoIntervalMinutes);
      return target === null ? null : $t("remoteRow.nextBisync", { values: { when: formatCountdown(target, $now) } });
    }
    return null;
  });

  let panelOpen = $state(false);

  let rootEl: HTMLLIElement | undefined = $state();
  let highlighted = $state(false);

  // Cliccando una voce "Configura" o un avviso nel menu della tray, il
  // backend porta la finestra in primo piano ed emette questo evento —
  // scorriamo fino a questa riga e la evidenziamo, aprendo anche il
  // pannello se veniva da un avviso (vedi tray.rs::focus_remote).
  // L'evidenziazione resta finché la finestra non torna nascosta in tray
  // (evento separato, vedi sotto) invece di sparire dopo un timeout
  // arbitrario — l'utente potrebbe metterci più di qualche secondo a
  // trovare/leggere la riga giusta.
  $effect(() => {
    let cancelled = false;
    let unlistenFocus: (() => void) | undefined;
    let unlistenHidden: (() => void) | undefined;

    listen<{ remote: string; openHistory: boolean }>("rclone-easy://tray-focus-remote", (event) => {
      if (event.payload.remote !== remoteName) return;
      rootEl?.scrollIntoView({ behavior: "smooth", block: "center" });
      highlighted = true;
      if (event.payload.openHistory) panelOpen = true;
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenFocus = fn;
    });

    listen("rclone-easy://window-hidden", () => {
      highlighted = false;
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenHidden = fn;
    });

    return () => {
      cancelled = true;
      unlistenFocus?.();
      unlistenHidden?.();
    };
  });

  // --- Remote stesso (eliminazione — resta un'azione a livello di remote,
  // non di servizio, quindi fuori dal pannello unico). ---
  type RemoteUsage = { mountName: string | null; backupName: string | null; bisyncName: string | null };
  let deleteRemoteModalOpen = $state(false);
  let remoteUsage = $state<RemoteUsage | null>(null);
  let deletingRemote = $state(false);
  let deleteRemoteError = $state<string | null>(null);

  async function openDeleteRemoteModal() {
    deleteRemoteError = null;
    remoteUsage = null;
    try {
      remoteUsage = await invoke<RemoteUsage>("remote_usage", { name: remoteName });
    } catch (error) {
      // L'elenco di cosa verrebbe eliminato insieme al remote è solo
      // informativo: se la richiesta fallisce si mostra comunque il modal
      // di conferma, semplicemente senza quel dettaglio.
    }
    deleteRemoteModalOpen = true;
  }

  async function confirmDeleteRemote() {
    deletingRemote = true;
    deleteRemoteError = null;
    try {
      await invoke("delete_remote_cascade", { name: remoteName });
      deleteRemoteModalOpen = false;
      onDeleted?.();
    } catch (error) {
      deleteRemoteError = String(error);
    } finally {
      deletingRemote = false;
    }
  }
</script>

<li class="remote-row" class:highlighted bind:this={rootEl}>
  <div class="row-main">
    <div class="remote-info">
      <span class="remote-name">{remoteName}</span>
      {#if lastAttempt === null}
        <span class="last-op muted">{$t("remoteRow.noOpYet")}</span>
      {:else if lastAttempt.outcome === "ok"}
        <span class="last-op">{$t("remoteRow.lastOpSuccess", { values: { when: formatWhen(lastAttempt.whenUnix) } })}</span>
      {:else if lastAttempt.outcome === "conflict"}
        <span class="last-op conflict">{$t("remoteRow.lastOpConflict", { values: { when: formatWhen(lastAttempt.whenUnix) } })}</span>
      {:else}
        <span class="last-op failed">
          {$t("remoteRow.lastOpFailed", { values: { when: formatWhen(lastAttempt.whenUnix), reason: truncate(lastAttempt.reason ?? "") } })}
        </span>
      {/if}
      {#if countdownText}
        <span class="last-op countdown">{countdownText}</span>
      {/if}
      {#if deleteRemoteError}
        <span class="error">✗ {deleteRemoteError}</span>
      {/if}
    </div>
    <div class="row-actions">
      <button type="button" class="service-pill" class:mount={displayService === "mount"} class:backup={displayService === "backup"} class:bisync={displayService === "bisync"} onclick={() => (panelOpen = true)}>
        {#if displayService}
          <Icon kind={displayService} />
          {SERVICE_LABELS[displayService]}
        {:else}
          <Icon kind="add" />
          {$t("remoteRow.notConfigured")}
        {/if}
      </button>
      <span class="separator"></span>
      <a class="icon-button" href={`/modifica-remote/${encodeURIComponent(remoteName)}`} title={$t("remoteRow.editRemote")}>
        <Icon kind="edit" />
      </a>
      <button
        type="button"
        class="icon-button action-delete"
        title={$t("remoteRow.deleteRemote")}
        onclick={openDeleteRemoteModal}
        disabled={deletingRemote}
      >
        <Icon kind="delete" />
      </button>
    </div>
  </div>
</li>

<Modal bind:open={panelOpen} title={$t("remotePanel.title", { values: { remote: remoteName } })}>
  <RemotePanel {remoteName} {mountEntry} {syncJob} {bisyncJob} {onRefresh} />
</Modal>

<Modal bind:open={deleteRemoteModalOpen} title={$t("remoteRow.deleteTitle", { values: { remote: remoteName } })}>
  <div class="modal-form">
    <p>{$t("remoteRow.deleteConfirmQuestion")}</p>
    {#if remoteUsage && (remoteUsage.mountName || remoteUsage.backupName || remoteUsage.bisyncName)}
      <div class="conflict-box">
        <strong>{$t("remoteRow.alsoDeleted")}</strong>
        <ul>
          {#if remoteUsage.mountName}<li>{$t("remoteRow.mountNamed", { values: { name: remoteUsage.mountName } })}</li>{/if}
          {#if remoteUsage.backupName}<li>{$t("remoteRow.backupNamed", { values: { name: remoteUsage.backupName } })}</li>{/if}
          {#if remoteUsage.bisyncName}<li>{$t("remoteRow.bisyncNamed", { values: { name: remoteUsage.bisyncName } })}</li>{/if}
        </ul>
      </div>
    {/if}
    <p class="hint">{$t("remoteRow.irreversible")}</p>
    {#if deleteRemoteError}
      <p class="error">✗ {deleteRemoteError}</p>
    {/if}
    <div class="row-actions modal-actions">
      <button type="button" onclick={() => (deleteRemoteModalOpen = false)} disabled={deletingRemote}>{$t("common.cancel")}</button>
      <button type="button" class="btn-danger" onclick={confirmDeleteRemote} disabled={deletingRemote}>
        {deletingRemote ? $t("remoteRow.deleting") : $t("common.confirm")}
      </button>
    </div>
  </div>
</Modal>

<style>
.remote-row {
  border-radius: 8px;
  background-color: var(--bg-surface);
  box-shadow: var(--shadow-md);
  padding: 0.8em 1em;
  outline: 2px solid transparent;
  outline-offset: 2px;
  transition: outline-color 0.3s ease;
}

.remote-row.highlighted {
  outline-color: var(--accent);
  animation: remote-row-pulse 1.6s ease;
}

@keyframes remote-row-pulse {
  0%,
  100% {
    background-color: var(--bg-surface);
  }
  20% {
    background-color: var(--accent-bg);
  }
}

.row-main {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 1em;
  flex-wrap: wrap;
}

.remote-info {
  display: flex;
  flex-direction: column;
  gap: 0.15em;
  min-width: 0;
}

.remote-name {
  font-weight: 600;
}

.last-op {
  font-size: 0.8em;
  color: var(--text-muted);
}

.last-op.muted {
  font-style: italic;
}

.last-op.countdown {
  color: var(--accent);
}

.last-op.failed {
  color: var(--error);
}

.last-op.conflict {
  color: var(--warning-text);
}

.row-actions {
  display: flex;
  align-items: center;
  gap: 0.5em;
  flex-shrink: 0;
}

.separator {
  width: 1px;
  height: 1.6em;
  background-color: var(--border-color-subtle);
  margin: 0 0.1em;
}

/* Un solo indicatore invece delle 3 icone mount/backup/bisync di prima —
   colorato per famiglia di servizio (mount = accento dell'app, backup =
   blu, bisync = viola), stesso principio cromatico di prima ma un solo
   controllo invece di tre, dato che sono comunque una scelta mutuamente
   esclusiva (audit UX 21/8/2026, punto B). */
.service-pill {
  display: inline-flex;
  align-items: center;
  gap: 0.5em;
  padding: 0.5em 0.9em;
  border-radius: 100px;
  border: 1px solid var(--border-color-subtle);
  background-color: var(--surface-tint);
  color: var(--text-muted);
  font-size: 0.85em;
  font-weight: 600;
  cursor: pointer;
  box-shadow: var(--shadow-icon-rest);
  transition: box-shadow 0.14s ease, transform 0.14s cubic-bezier(0.2, 0.8, 0.3, 1);
}

.service-pill:hover {
  transform: translateY(-1px);
  box-shadow: var(--shadow-icon-hover);
}

.service-pill.mount {
  color: var(--accent);
  background-color: var(--accent-bg);
}

.service-pill.backup {
  color: var(--blue);
  background-color: var(--blue-bg);
}

.service-pill.bisync {
  color: var(--violet);
  background-color: var(--violet-bg);
}

.icon-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 2.7em;
  height: 2.7em;
  font-size: 1rem;
  border-radius: 12px;
  border: 1px solid var(--border-color-subtle);
  background-color: var(--surface-tint);
  color: var(--text-color);
  cursor: pointer;
  text-decoration: none;
  box-shadow: var(--shadow-icon-rest);
  transition: background-color 0.14s ease, border-color 0.14s ease, color 0.14s ease, box-shadow 0.14s ease,
    transform 0.14s cubic-bezier(0.2, 0.8, 0.3, 1);
}

.icon-button:hover:not(:disabled) {
  transform: translateY(-2px);
  box-shadow: var(--shadow-icon-hover);
}

.icon-button:active:not(:disabled) {
  transform: translateY(0);
}

.icon-button:disabled {
  cursor: default;
  opacity: 0.5;
}

.icon-button.action-delete:hover:not(:disabled),
.icon-button.action-delete:focus-visible {
  color: var(--bg-surface);
  background-color: var(--status-red);
  border-color: transparent;
}
</style>
