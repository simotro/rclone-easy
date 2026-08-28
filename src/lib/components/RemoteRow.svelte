<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { goto } from "$app/navigation";
  import Modal from "./Modal.svelte";
  import Icon from "./Icon.svelte";
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

  // Un mount conta come "attivo" anche solo con l'auto-mount all'avvio
  // abilitato, non solo se montato adesso — stessa definizione del backend
  // (activity.rs::active_service_for_remote), duplicata anche in
  // RemotePanel.svelte per lo stesso motivo (vedi lì per il perché).
  let activeService = $derived.by<ServiceKind | null>(() => {
    if (mountEntry?.mounted || mountEntry?.autoMount) return "mount";
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
    if (displayService === "mount" && mountEntry) {
      const latest = mountEntry.history[0];
      if (!latest) return null;
      return { whenUnix: latest.whenUnix, outcome: latest.success ? "ok" : "failed", reason: latest.success ? null : latest.message };
    }
    if (displayService === "backup" && syncJob) {
      const latest = syncJob.history[0];
      if (!latest) return null;
      return { whenUnix: latest.whenUnix, outcome: latest.success ? "ok" : "failed", reason: latest.success ? null : latest.message };
    }
    if (displayService === "bisync" && bisyncJob) {
      const latest = bisyncJob.history[0];
      if (!latest) return null;
      if (!latest.success) return { whenUnix: latest.whenUnix, outcome: "failed", reason: latest.message };
      if (latest.conflictPaths.length > 0) return { whenUnix: latest.whenUnix, outcome: "conflict", reason: null };
      return { whenUnix: latest.whenUnix, outcome: "ok", reason: null };
    }
    return null;
  });

  // Segnale indipendente dall'esito del run: rclone può ignorare nomi
  // duplicati sul remote (in pratica solo Google Drive tra i backend comuni)
  // anche in un run altrimenti riuscito — senza questo l'informazione
  // restava sepolta nel log grezzo.
  let duplicateNamesCount = $derived(displayService === "bisync" ? (bisyncJob?.history[0]?.duplicateNames.length ?? 0) : 0);

  function formatWhen(whenUnix: number): string {
    return new Date(whenUnix * 1000).toLocaleString();
  }

  async function openMountFolder(mountPoint: string) {
    await invoke("open_mount_folder", { mountPoint });
  }

  // "Esegui ora"/"Monta"/"Smonta" — spostati qui dal tab "Esegui e stato"
  // del pannello, eliminato perché ridondante con quello che questa riga
  // già mostra (stato) e con la Cronologia (esito, log, recupero da
  // conflitti bisync bloccati) (Simone, 22/8/2026). Un link di testo in
  // coda alla riga di stato invece di un pulsante a parte accanto alla
  // pill — niente forma/colore in più che compete con la pill, niente
  // disallineamento tra righe di larghezza diversa (idea di Simone,
  // 22/8/2026, dopo aver trovato il primo pulsante troppo "rumoroso").
  let quickActionBusy = $state(false);
  let quickActionError = $state<string | null>(null);
  let quickActionDisabled = $derived(Boolean(syncJob?.isRunning || syncJob?.isDryRunning || bisyncJob?.isRunning || bisyncJob?.isDryRunning));
  let showQuickActionInline = $derived(Boolean(displayService) && !quickActionDisabled);
  let quickActionLabel = $derived.by(() => {
    if (displayService === "mount") return mountEntry?.mounted ? $t("remoteRow.unmount") : $t("remoteRow.mountAndOpen");
    return $t("remoteRow.runNow");
  });

  async function runQuickAction() {
    if (!displayService || quickActionBusy || quickActionDisabled) return;
    quickActionBusy = true;
    quickActionError = null;
    try {
      if (displayService === "mount" && mountEntry) {
        if (mountEntry.mounted) await invoke("unmount_now", { name: mountEntry.name });
        else await invoke("mount_now_and_open", { name: mountEntry.name });
      } else if (displayService === "backup") {
        await invoke("run_job", { name: remoteName });
      } else if (displayService === "bisync") {
        await invoke("run_bisync_job", { name: remoteName });
      }
    } catch (error) {
      quickActionError = String(error);
    } finally {
      quickActionBusy = false;
      await onRefresh?.();
    }
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

  // Cliccando una voce "Configura" o un avviso nel menu della tray, il
  // backend porta la finestra in primo piano ed emette un evento — gestito
  // ora in +layout.svelte (naviga direttamente alla pagina del remote,
  // niente più evidenziazione della riga: la riga non è più "dietro" a
  // niente, la pagina di destinazione dice già da sola di quale remote si
  // tratta). Vedi tray.rs::focus_remote.

  // --- Menu "⋮" (Modifica/Elimina) — sostituisce le due icone singole
  // affiancate alla pill di stato, disomogenee con quella (audit UX,
  // Simone 21/8/2026, punti 7/9/10). Un solo listener a livello di
  // documento invece che condizionato all'apertura: attaccato fin dal
  // mount, così il click che APRE il menu (che raggiunge document in
  // bubbling DOPO aver aperto) non rischia di richiuderlo subito — il
  // controllo `moreMenuEl.contains` lo esclude comunque perché il pulsante
  // che apre sta dentro lo stesso contenitore.
  let moreMenuOpen = $state(false);
  let moreMenuEl: HTMLDivElement | undefined = $state();

  $effect(() => {
    function onDocClick(event: MouseEvent) {
      if (moreMenuOpen && moreMenuEl && !moreMenuEl.contains(event.target as Node)) moreMenuOpen = false;
    }
    function onKeydown(event: KeyboardEvent) {
      if (event.key === "Escape") moreMenuOpen = false;
    }
    document.addEventListener("click", onDocClick);
    document.addEventListener("keydown", onKeydown);
    return () => {
      document.removeEventListener("click", onDocClick);
      document.removeEventListener("keydown", onKeydown);
    };
  });

  // --- Remote stesso (eliminazione — resta un'azione a livello di remote,
  // non di servizio, quindi fuori dal pannello unico). ---
  let deleteRemoteModalOpen = $state(false);
  let deletingRemote = $state(false);
  let deleteRemoteError = $state<string | null>(null);

  function openDeleteRemoteModal() {
    deleteRemoteError = null;
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

<li class="remote-row">
  <div class="row-main">
    <div class="remote-info">
      <span class="remote-name">{remoteName}</span>
      {#snippet quickActionLink()}
        {#if showQuickActionInline}
          <button type="button" class="inline-action-link" onclick={runQuickAction} disabled={quickActionBusy}>
            {quickActionBusy ? $t("common.inProgress") : quickActionLabel}
          </button>
        {/if}
      {/snippet}
      {#if displayService === "mount"}
        {#if mountEntry?.mounted}
          <span class="last-op">
            {$t("remoteRow.mountedOn")}
            <button type="button" class="path-link" onclick={() => openMountFolder(mountEntry.mountPoint)}>{mountEntry.mountPoint}</button>
            {@render quickActionLink()}
          </span>
        {:else if lastAttempt?.outcome === "failed"}
          <span class="last-op failed">
            {$t("remoteRow.lastOpFailed", { values: { when: formatWhen(lastAttempt.whenUnix), reason: truncate(lastAttempt.reason ?? "") } })}
            {@render quickActionLink()}
          </span>
        {:else}
          <span class="last-op">
            {$t("remoteRow.currentlyNotMounted")}
            {@render quickActionLink()}
          </span>
        {/if}
      {:else if lastAttempt === null}
        <span class="last-op">
          {$t("remoteRow.noOpYet")}
          {#if !countdownText}{@render quickActionLink()}{/if}
        </span>
      {:else if lastAttempt.outcome === "ok"}
        <span class="last-op">
          {$t("remoteRow.lastOpSuccess", { values: { when: formatWhen(lastAttempt.whenUnix) } })}
          {#if !countdownText}{@render quickActionLink()}{/if}
        </span>
      {:else if lastAttempt.outcome === "conflict"}
        <span class="last-op conflict">
          {$t("remoteRow.lastOpConflict", { values: { when: formatWhen(lastAttempt.whenUnix) } })}
          {#if !countdownText}{@render quickActionLink()}{/if}
        </span>
      {:else}
        <span class="last-op failed">
          {$t("remoteRow.lastOpFailed", { values: { when: formatWhen(lastAttempt.whenUnix), reason: truncate(lastAttempt.reason ?? "") } })}
          {#if !countdownText}{@render quickActionLink()}{/if}
        </span>
      {/if}
      {#if countdownText}
        <span class="last-op countdown">
          {countdownText}
          {@render quickActionLink()}
        </span>
      {/if}
      {#if duplicateNamesCount > 0}
        <span class="last-op conflict">
          {$t("remoteRow.duplicateNamesWarning", { values: { count: duplicateNamesCount } })}
        </span>
      {/if}
      {#if deleteRemoteError}
        <span class="error">✗ {deleteRemoteError}</span>
      {/if}
      {#if quickActionError}
        <span class="error">✗ {truncate(quickActionError)}</span>
      {/if}
    </div>
    <div class="row-actions">
      <button
        type="button"
        class="service-pill"
        class:mount={displayService === "mount"}
        class:backup={displayService === "backup"}
        class:bisync={displayService === "bisync"}
        onclick={() => goto(`/remote/${encodeURIComponent(remoteName)}`)}
      >
        {#if displayService}
          <Icon kind={displayService} />
          {SERVICE_LABELS[displayService]}
        {:else}
          {$t("remoteRow.configure")}
        {/if}
      </button>
      <span class="separator"></span>
      <div class="more-menu" bind:this={moreMenuEl}>
        <button
          type="button"
          class="icon-button"
          title={$t("remoteRow.moreActions")}
          aria-haspopup="menu"
          aria-expanded={moreMenuOpen}
          onclick={() => (moreMenuOpen = !moreMenuOpen)}
        >
          <Icon kind="more" />
        </button>
        {#if moreMenuOpen}
          <div class="more-menu-panel" role="menu">
            <a
              class="more-menu-item"
              role="menuitem"
              href={`/modifica-remote/${encodeURIComponent(remoteName)}`}
              onclick={() => (moreMenuOpen = false)}
            >
              <Icon kind="edit" />
              {$t("remoteRow.editRemote")}
            </a>
            <button
              type="button"
              class="more-menu-item danger"
              role="menuitem"
              onclick={() => {
                moreMenuOpen = false;
                openDeleteRemoteModal();
              }}
              disabled={deletingRemote}
            >
              <Icon kind="delete" />
              {$t("remoteRow.deleteRemote")}
            </button>
          </div>
        {/if}
      </div>
    </div>
  </div>
</li>

<Modal bind:open={deleteRemoteModalOpen} title={$t("remoteRow.deleteTitle", { values: { remote: remoteName } })}>
  <div class="modal-form">
    <p>{$t("remoteRow.deleteConfirmQuestion")}</p>
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

.last-op.countdown {
  color: var(--accent);
}

.last-op.failed {
  color: var(--error);
}

.last-op.conflict {
  color: var(--warning-text);
}

.path-link {
  background: none;
  border: none;
  box-shadow: none;
  padding: 0;
  margin: 0;
  font: inherit;
  color: var(--accent);
  text-decoration: underline;
  cursor: pointer;
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

.inline-action-link {
  background: none;
  border: none;
  box-shadow: none;
  padding: 0;
  margin: 0;
  font: inherit;
  font-weight: 600;
  color: var(--accent);
  text-decoration: underline;
  cursor: pointer;
}

.inline-action-link:disabled {
  color: inherit;
  text-decoration: none;
  cursor: default;
  opacity: 0.6;
}

/* Stessa "famiglia" visiva della pill accanto (stesso raggio angoli
   completamente arrotondato, stesso font-size, stesso sollevamento in
   hover) invece di un quadrato dagli angoli appena smussati — prima le due
   forme non sembravano appartenere allo stesso controllo (Simone,
   23/8/2026). */
.icon-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  aspect-ratio: 1;
  padding: 0.6em;
  font-size: 0.85em;
  border-radius: 100px;
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
  transform: translateY(-1px);
  box-shadow: var(--shadow-icon-hover);
}

.icon-button:active:not(:disabled) {
  transform: translateY(0);
}

.icon-button:disabled {
  cursor: default;
  opacity: 0.5;
}

.more-menu {
  position: relative;
}

.more-menu-panel {
  position: absolute;
  top: calc(100% + 0.4em);
  right: 0;
  z-index: 10;
  display: flex;
  flex-direction: column;
  min-width: 11em;
  padding: 0.35em;
  border-radius: 10px;
  border: 1px solid var(--border-color-subtle);
  background-color: var(--bg-surface);
  box-shadow: var(--shadow-icon-hover);
}

.more-menu-item {
  display: flex;
  align-items: center;
  gap: 0.6em;
  width: 100%;
  padding: 0.55em 0.7em;
  border: none;
  border-radius: 7px;
  background: none;
  box-shadow: none;
  color: var(--text-color);
  text-decoration: none;
  font-size: 0.9em;
  text-align: left;
  cursor: pointer;
}

.more-menu-item:hover:not(:disabled) {
  background-color: var(--surface-tint);
}

.more-menu-item.danger {
  color: var(--status-red);
}

.more-menu-item.danger:hover:not(:disabled) {
  color: var(--bg-surface);
  background-color: var(--status-red);
}

.more-menu-item:disabled {
  opacity: 0.5;
  cursor: default;
}
</style>
