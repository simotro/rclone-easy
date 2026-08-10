<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
  import Modal from "./Modal.svelte";
  import Icon from "./Icon.svelte";
  import StatusDot from "./StatusDot.svelte";
  import RemoteFolderPicker from "./RemoteFolderPicker.svelte";
  import { now, formatCountdown, nextRunAtMs } from "$lib/now";
  import type { MountEntry, SyncJob, BisyncJob, BisyncRunEntry } from "$lib/types";

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

  // La parte di un fs `remoto:sottocartella` dopo il nome del remote —
  // "" per la radice. Usata per precompilare il selettore di cartella
  // remota con quanto già scelto in precedenza quando si riapre un modal.
  function remoteSubPathOf(fs: string): string {
    return fs.startsWith(prefix) ? fs.slice(prefix.length) : "";
  }

  // Un solo RemoteFolderPicker condiviso da mount/backup/bisync invece di
  // tre istanze: al più un modal di configurazione è aperto alla volta,
  // quindi al più un "Sfoglia…" può essere in corso.
  let remotePickerOpen = $state(false);
  let remotePickerOnSelect: ((path: string) => void) | null = $state(null);

  function openRemotePicker(onSelect: (path: string) => void) {
    remotePickerOnSelect = onSelect;
    remotePickerOpen = true;
  }

  type ActiveService = "mount" | "backup" | "bisync" | null;

  let activeService = $derived.by<ActiveService>(() => {
    if (mountEntry?.mounted) return "mount";
    if (syncJob?.autoIntervalMinutes !== null && syncJob !== null) return "backup";
    if (bisyncJob?.autoIntervalMinutes !== null && bisyncJob !== null) return "bisync";
    return null;
  });

  type StatusColor = "green" | "yellow" | "red" | "grey";

  let statusColor = $derived.by<StatusColor>(() => {
    if (activeService === "mount") {
      const latest = mountEntry?.history[0];
      if (!latest) return "grey";
      return latest.success ? "green" : "red";
    }
    if (activeService === "backup") {
      const latest = syncJob?.history[0];
      if (!latest) return "grey";
      return latest.success ? "green" : "red";
    }
    if (activeService === "bisync") {
      const latest = bisyncJob?.history[0];
      if (!latest) return "grey";
      if (!latest.success) return "red";
      return latest.conflictPaths.length > 0 ? "yellow" : "green";
    }
    return "grey";
  });

  let lastSuccessfulOp = $derived.by<number | null>(() => {
    const candidates: number[] = [];
    const mountSuccess = mountEntry?.history.find((h) => h.success);
    if (mountSuccess) candidates.push(mountSuccess.whenUnix);
    const syncSuccess = syncJob?.history.find((h) => h.success);
    if (syncSuccess) candidates.push(syncSuccess.whenUnix);
    const bisyncSuccess = bisyncJob?.history.find((h) => h.success);
    if (bisyncSuccess) candidates.push(bisyncSuccess.whenUnix);
    if (candidates.length === 0) return null;
    return Math.max(...candidates);
  });

  function formatWhen(whenUnix: number): string {
    return new Date(whenUnix * 1000).toLocaleString();
  }

  // Conto alla rovescia verso la prossima esecuzione automatica — solo per
  // backup/bisync (il mount non ha un "prossimo giro", resta montato finché
  // non lo si smonta), e solo per il servizio effettivamente attivo: la
  // mutua esclusività garantisce che al massimo uno tra backup e bisync
  // possa avere l'automazione accesa per questo remote alla volta.
  let countdownText = $derived.by<string | null>(() => {
    if (activeService === "backup" && syncJob) {
      if (syncJob.isRunning) return "Backup in corso…";
      if (syncJob.history.length === 0) return "Primo backup a momenti";
      const target = nextRunAtMs(syncJob.history[0].whenUnix, syncJob.autoIntervalMinutes);
      return target === null ? null : `Prossimo backup ${formatCountdown(target, $now)}`;
    }
    if (activeService === "bisync" && bisyncJob) {
      if (bisyncJob.isRunning) return "Sincronizzazione in corso…";
      if (bisyncJob.history.length === 0) return "Prima sincronizzazione a momenti";
      const target = nextRunAtMs(bisyncJob.history[0].whenUnix, bisyncJob.autoIntervalMinutes);
      return target === null ? null : `Prossima sincronizzazione ${formatCountdown(target, $now)}`;
    }
    return null;
  });

  let mountModalOpen = $state(false);
  let backupModalOpen = $state(false);
  let bisyncModalOpen = $state(false);
  let historyModalOpen = $state(false);

  let rootEl: HTMLLIElement | undefined = $state();
  let highlighted = $state(false);

  // Cliccando una voce "Configura" o un avviso nel menu della tray, il
  // backend porta la finestra in primo piano ed emette questo evento —
  // scorriamo fino a questa riga e la evidenziamo, aprendo anche la
  // cronologia se veniva da un avviso (vedi tray.rs::focus_remote).
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
      if (event.payload.openHistory) historyModalOpen = true;
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

  type ServiceKind = "mount" | "backup" | "bisync";
  const SERVICE_LABELS: Record<ServiceKind, string> = {
    mount: "il mount",
    backup: "il backup",
    bisync: "la sincronizzazione bidirezionale",
  };

  // Spegne il servizio indicato per davvero: per il mount smonta prima se
  // necessario (un mount ancora montato non può considerarsi disattivato),
  // per backup/bisync azzera l'intervallo automatico. Usata sia dallo
  // switch tra servizi (conflictModal) sia dal pulsante "Disabilita" nei
  // rispettivi modal.
  async function disableMount() {
    if (!mountEntry) return;
    if (mountEntry.mounted) {
      await invoke("unmount_now", { name: mountEntry.name });
    }
    if (mountEntry.autoMount) {
      await invoke("update_mount", {
        oldName: mountEntry.name,
        name: mountEntry.name,
        remote: mountEntry.remote,
        mountPoint: mountEntry.mountPoint,
        autoMount: false,
      });
    }
    await onRefresh?.();
  }

  async function disableBackup() {
    if (!syncJob) return;
    await invoke("update_job", {
      oldName: syncJob.name,
      name: syncJob.name,
      source: syncJob.source,
      destination: syncJob.destination,
      autoIntervalMinutes: null,
      propagateDeletions: syncJob.propagateDeletions,
    });
    await onRefresh?.();
  }

  async function disableBisync() {
    if (!bisyncJob) return;
    await invoke("update_bisync_job", {
      oldName: bisyncJob.name,
      name: bisyncJob.name,
      path1: bisyncJob.path1,
      path2: bisyncJob.path2,
      autoIntervalMinutes: null,
    });
    await onRefresh?.();
  }

  async function disableActiveService(kind: ServiceKind) {
    if (kind === "mount") await disableMount();
    else if (kind === "backup") await disableBackup();
    else await disableBisync();
  }

  // Aperti direttamente da onServiceIconClick quando non c'è conflitto, o
  // dal modal di conferma dopo aver spento il servizio attivo.
  function openModalFor(kind: ServiceKind) {
    if (kind === "mount") openMountModal();
    else if (kind === "backup") openBackupModal();
    else openBisyncModal();
  }

  let conflictModalOpen = $state(false);
  let conflictTargetKind = $state<ServiceKind | null>(null);
  let conflictBusy = $state(false);
  let conflictError = $state<string | null>(null);

  // Punto d'ingresso dei 3 pulsanti icona mount/backup/bisync: se un
  // servizio diverso è già attivo per questo remote, avvisa PRIMA di far
  // compilare qualunque campo — non al momento del salvataggio, quando
  // l'utente avrebbe già perso tempo a configurare (richiesta esplicita:
  // l'avviso deve arrivare prima della possibilità di inserire i dati).
  function onServiceIconClick(kind: ServiceKind) {
    if (activeService !== null && activeService !== kind) {
      conflictTargetKind = kind;
      conflictError = null;
      conflictModalOpen = true;
    } else {
      openModalFor(kind);
    }
  }

  async function confirmConflictSwitch() {
    if (!conflictTargetKind || activeService === null) return;
    conflictBusy = true;
    conflictError = null;
    try {
      await disableActiveService(activeService);
      const kind = conflictTargetKind;
      conflictModalOpen = false;
      openModalFor(kind);
    } catch (error) {
      conflictError = String(error);
    } finally {
      conflictBusy = false;
    }
  }

  // --- Mount ---
  let mountBusy = $state(false);
  let mountError = $state<string | null>(null);
  let mountFormPoint = $state("");
  let mountFormAuto = $state(false);
  let mountFormRemotePath = $state("");

  function openMountModal() {
    mountFormPoint = mountEntry?.mountPoint ?? "";
    mountFormAuto = mountEntry?.autoMount ?? false;
    mountFormRemotePath = mountEntry ? remoteSubPathOf(mountEntry.remote) : "";
    mountError = null;
    mountModalOpen = true;
  }

  async function pickMountFolder() {
    const selected = await openFolderDialog({ directory: true, multiple: false, title: "Scegli o crea una cartella locale" });
    if (typeof selected === "string") mountFormPoint = selected;
  }

  // Unico pulsante del modal: salva la configurazione (creandola se non
  // esiste ancora), monta e apre la cartella nel file manager di sistema —
  // a differenza di backup/bisync non ha senso separare "salva" da "esegui
  // ora", perché per un mount l'unica azione sensata è "sii montato qui", e
  // aprire subito la cartella è l'unica conferma visiva diretta che il
  // montaggio è riuscito (`mount_now_and_open` in mounts.rs). Se il
  // percorso è cambiato rispetto a un mount ancora risultato montato dal
  // vivo (stato stantio nel form, difesa contro un disallineamento
  // esterno: il pulsante stesso è visibile solo quando mountEntry non è
  // montato), smonta prima dal vecchio percorso: `update_mount` rifiuta
  // altrimenti. Chiude il modal solo in caso di successo.
  async function confirmMount() {
    if (mountFormPoint.trim() === "") return;
    mountBusy = true;
    mountError = null;
    try {
      const remote = `${prefix}${mountFormRemotePath}`;
      const mountPoint = mountFormPoint.trim();
      if (mountEntry?.mounted && mountEntry.mountPoint !== mountPoint) {
        await invoke("unmount_now", { name: mountEntry.name });
      }
      if (mountEntry) {
        await invoke("update_mount", { oldName: mountEntry.name, name: remoteName, remote, mountPoint, autoMount: mountFormAuto });
      } else {
        await invoke("create_mount", { name: remoteName, remote, mountPoint, autoMount: mountFormAuto });
      }
      await invoke("mount_now_and_open", { name: remoteName });
      await onRefresh?.();
      mountModalOpen = false;
    } catch (error) {
      mountError = String(error);
    } finally {
      mountBusy = false;
    }
  }

  // Smonta senza toccare la configurazione salvata (niente delete_mount):
  // resta pronta per un prossimo "Monta e apri" sullo stesso percorso.
  // Chiude il modal solo in caso di successo.
  async function unmountOnly() {
    if (!mountEntry) return;
    mountBusy = true;
    mountError = null;
    try {
      await invoke("unmount_now", { name: mountEntry.name });
      await onRefresh?.();
      mountModalOpen = false;
    } catch (error) {
      mountError = String(error);
    } finally {
      mountBusy = false;
    }
  }

  // --- Backup ---
  type Direction = "toRemote" | "fromRemote";
  let backupBusy = $state(false);
  let backupError = $state<string | null>(null);
  let backupFormDirection = $state<Direction>("toRemote");
  let backupFormLocalPath = $state("");
  let backupFormAutoEnabled = $state(true);
  let backupFormAutoInterval = $state(15);
  let backupFormPropagateDeletions = $state(false);
  let backupRunResult = $state<{ success: boolean; message: string } | null>(null);
  // Stato del job al momento dell'apertura del modal, non quello live: usato
  // solo per decidere se mostrare "Disabilita" — altrimenti apparirebbe
  // subito dopo aver appena abilitato un job nello stesso modal, proprio
  // nel momento in cui non deve esserci.
  let backupWasActiveOnOpen = $state(false);
  let propagateDeletionsWarningOpen = $state(false);
  let backupFormRemotePath = $state("");

  function directionOf(job: SyncJob): Direction {
    return job.source.startsWith(prefix) ? "fromRemote" : "toRemote";
  }
  function localPathOf(job: SyncJob): string {
    return directionOf(job) === "toRemote" ? job.source : job.destination;
  }
  function remoteFsOf(job: SyncJob): string {
    return directionOf(job) === "toRemote" ? job.destination : job.source;
  }

  function openBackupModal() {
    if (syncJob) {
      backupFormDirection = directionOf(syncJob);
      backupFormLocalPath = localPathOf(syncJob);
      backupFormAutoEnabled = syncJob.autoIntervalMinutes !== null;
      backupFormAutoInterval = syncJob.autoIntervalMinutes ?? 15;
      backupFormPropagateDeletions = syncJob.propagateDeletions;
      backupFormRemotePath = remoteSubPathOf(remoteFsOf(syncJob));
    } else {
      backupFormDirection = "toRemote";
      backupFormLocalPath = "";
      backupFormAutoEnabled = true;
      backupFormAutoInterval = 15;
      backupFormPropagateDeletions = false;
      backupFormRemotePath = "";
    }
    backupWasActiveOnOpen = syncJob?.autoIntervalMinutes !== null && syncJob !== null;
    backupError = null;
    backupRunResult = null;
    backupModalOpen = true;
  }

  async function pickBackupFolder() {
    const selected = await openFolderDialog({ directory: true, multiple: false, title: "Scegli una cartella" });
    if (typeof selected === "string") backupFormLocalPath = selected;
  }

  // Il checkbox non spunta mai da solo: attivare "Propaga cancellazioni" è
  // un cambio di comportamento rischioso (può cancellare file nella
  // destinazione), quindi passa sempre da un avviso esplicito prima. Per
  // disattivarlo invece nessun avviso: tornare al comportamento sicuro non
  // ha bisogno di conferma. Lascia che il checkbox si spunti/tolga da solo
  // (bind:checked) e corregge DOPO in onchange, invece di intercettare il
  // click con preventDefault: più robusto, non dipende da come questo
  // specifico webview gestisce l'annullamento dell'azione di default di un
  // checkbox.
  function onPropagateDeletionsChange() {
    if (backupFormPropagateDeletions) {
      backupFormPropagateDeletions = false;
      propagateDeletionsWarningOpen = true;
    }
  }

  function confirmPropagateDeletions() {
    backupFormPropagateDeletions = true;
    propagateDeletionsWarningOpen = false;
  }

  // Salva sempre il form prima di eseguire qualunque azione: usata sia da
  // saveBackup sia da runBackupNow, così "Esegui ora" non gira mai
  // sull'ultima versione salvata ignorando una modifica non ancora
  // confermata nel form (es. una nuova cartella locale appena scelta) —
  // stesso principio già seguito dal mount, che ha un solo pulsante
  // "salva e monta" invece di due azioni separate.
  async function persistBackup() {
    const remoteFs = `${prefix}${backupFormRemotePath}`;
    const source = backupFormDirection === "toRemote" ? backupFormLocalPath.trim() : remoteFs;
    const destination = backupFormDirection === "toRemote" ? remoteFs : backupFormLocalPath.trim();
    const autoIntervalMinutes = backupFormAutoEnabled ? backupFormAutoInterval : null;
    const propagateDeletions = backupFormPropagateDeletions;
    if (syncJob) {
      await invoke("update_job", {
        oldName: syncJob.name,
        name: remoteName,
        source,
        destination,
        autoIntervalMinutes,
        propagateDeletions,
      });
    } else {
      await invoke("create_job", { name: remoteName, source, destination, autoIntervalMinutes, propagateDeletions });
    }
  }

  async function saveBackup() {
    if (backupFormLocalPath.trim() === "") return;
    backupBusy = true;
    backupError = null;
    try {
      await persistBackup();
      await onRefresh?.();
      backupModalOpen = false;
    } catch (error) {
      backupError = String(error);
    } finally {
      backupBusy = false;
    }
  }

  async function runBackupNow() {
    if (backupFormLocalPath.trim() === "") return;
    backupBusy = true;
    backupError = null;
    backupRunResult = null;
    try {
      await persistBackup();
      await invoke("run_job", { name: remoteName });
      backupRunResult = { success: true, message: "" };
    } catch (error) {
      backupRunResult = { success: false, message: String(error) };
    } finally {
      backupBusy = false;
      await onRefresh?.();
    }
  }

  // Disabilita un backup attivo e chiude subito il modal — non lascia la
  // possibilità di cliccare "Salva e chiudi" subito dopo per errore, che
  // con il form ancora sull'intervallo precedente riabiliterebbe
  // l'automazione appena spenta invece di lasciarla disattivata.
  async function disableBackupAndClose() {
    backupBusy = true;
    backupError = null;
    try {
      await disableBackup();
      backupModalOpen = false;
    } catch (error) {
      backupError = String(error);
    } finally {
      backupBusy = false;
    }
  }

  // --- Bisync ---
  let bisyncBusy = $state(false);
  let bisyncError = $state<string | null>(null);
  let bisyncFormLocalPath = $state("");
  let bisyncFormAutoEnabled = $state(true);
  let bisyncFormAutoInterval = $state(15);
  let bisyncRunResult = $state<BisyncRunEntry | null>(null);
  let bisyncWasActiveOnOpen = $state(false);
  let bisyncFormRemotePath = $state("");

  function bisyncLocalPathOf(job: BisyncJob): string {
    return job.path1.startsWith(prefix) ? job.path2 : job.path1;
  }
  function bisyncRemoteFsOf(job: BisyncJob): string {
    return job.path1.startsWith(prefix) ? job.path1 : job.path2;
  }

  function openBisyncModal() {
    if (bisyncJob) {
      bisyncFormLocalPath = bisyncLocalPathOf(bisyncJob);
      bisyncFormAutoEnabled = bisyncJob.autoIntervalMinutes !== null;
      bisyncFormAutoInterval = bisyncJob.autoIntervalMinutes ?? 15;
      bisyncFormRemotePath = remoteSubPathOf(bisyncRemoteFsOf(bisyncJob));
    } else {
      bisyncFormLocalPath = "";
      bisyncFormAutoEnabled = true;
      bisyncFormAutoInterval = 15;
      bisyncFormRemotePath = "";
    }
    bisyncWasActiveOnOpen = bisyncJob?.autoIntervalMinutes !== null && bisyncJob !== null;
    bisyncError = null;
    bisyncRunResult = null;
    bisyncModalOpen = true;
  }

  async function pickBisyncFolder() {
    const selected = await openFolderDialog({ directory: true, multiple: false, title: "Scegli una cartella" });
    if (typeof selected === "string") bisyncFormLocalPath = selected;
  }

  // Stesso principio di persistBackup: salva sempre il form prima di
  // eseguire, così "Esegui ora" non gira mai su un percorso vecchio quando
  // il form ha una modifica non ancora confermata.
  async function persistBisync() {
    const path1 = bisyncFormLocalPath.trim();
    const path2 = `${prefix}${bisyncFormRemotePath}`;
    const autoIntervalMinutes = bisyncFormAutoEnabled ? bisyncFormAutoInterval : null;
    if (bisyncJob) {
      await invoke("update_bisync_job", { oldName: bisyncJob.name, name: remoteName, path1, path2, autoIntervalMinutes });
    } else {
      await invoke("create_bisync_job", { name: remoteName, path1, path2, autoIntervalMinutes });
    }
  }

  async function saveBisync() {
    if (bisyncFormLocalPath.trim() === "") return;
    bisyncBusy = true;
    bisyncError = null;
    try {
      await persistBisync();
      await onRefresh?.();
      bisyncModalOpen = false;
    } catch (error) {
      bisyncError = String(error);
    } finally {
      bisyncBusy = false;
    }
  }

  async function runBisyncNow() {
    if (bisyncFormLocalPath.trim() === "") return;
    bisyncBusy = true;
    bisyncError = null;
    bisyncRunResult = null;
    try {
      await persistBisync();
      bisyncRunResult = await invoke<BisyncRunEntry>("run_bisync_job", { name: remoteName });
    } catch (error) {
      bisyncError = String(error);
    } finally {
      bisyncBusy = false;
      await onRefresh?.();
    }
  }

  // Stesso principio di disableBackupAndClose, per la bisync.
  async function disableBisyncAndClose() {
    bisyncBusy = true;
    bisyncError = null;
    try {
      await disableBisync();
      bisyncModalOpen = false;
    } catch (error) {
      bisyncError = String(error);
    } finally {
      bisyncBusy = false;
    }
  }

  // --- Remote stesso ---
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
      {#if lastSuccessfulOp !== null}
        <span class="last-op">Ultima operazione riuscita: {formatWhen(lastSuccessfulOp)}</span>
      {:else}
        <span class="last-op muted">Nessuna operazione ancora eseguita</span>
      {/if}
      {#if countdownText}
        <span class="last-op countdown">{countdownText}</span>
      {/if}
      {#if deleteRemoteError}
        <span class="error">✗ {deleteRemoteError}</span>
      {/if}
    </div>
    <div class="row-actions">
      <button
        type="button"
        class="icon-button"
        class:active={activeService === "mount"}
        title="Mount: collega questo remote come cartella locale"
        onclick={() => onServiceIconClick("mount")}
      >
        <Icon kind="mount" />
      </button>
      <button
        type="button"
        class="icon-button"
        class:active={activeService === "backup"}
        title="Backup: sincronizzazione in una sola direzione"
        onclick={() => onServiceIconClick("backup")}
      >
        <Icon kind="backup" />
      </button>
      <button
        type="button"
        class="icon-button"
        class:active={activeService === "bisync"}
        title="Sincronizzazione bidirezionale"
        onclick={() => onServiceIconClick("bisync")}
      >
        <Icon kind="bisync" />
      </button>
      <button type="button" class="icon-button status-button" title="Stato e cronologia" onclick={() => (historyModalOpen = true)}>
        <StatusDot color={statusColor} />
      </button>
      <span class="separator"></span>
      <a class="icon-button" href={`/modifica-remote/${encodeURIComponent(remoteName)}`} title="Modifica remote">
        <Icon kind="edit" />
      </a>
      <button
        type="button"
        class="icon-button"
        title="Elimina remote"
        onclick={openDeleteRemoteModal}
        disabled={deletingRemote}
      >
        <Icon kind="delete" />
      </button>
    </div>
  </div>
</li>

<Modal bind:open={conflictModalOpen} title="Attenzione">
  <div class="modal-form">
    {#if activeService && conflictTargetKind}
      <p>
        Per questo remote hai già attivo {SERVICE_LABELS[activeService]}. Attivando {SERVICE_LABELS[conflictTargetKind]} verrà
        disabilitato {SERVICE_LABELS[activeService]}.
      </p>
      <p class="hint">Vuoi procedere?</p>
    {/if}
    {#if conflictError}
      <p class="error">✗ {conflictError}</p>
    {/if}
    <div class="row-actions modal-actions">
      <button type="button" onclick={() => (conflictModalOpen = false)} disabled={conflictBusy}>Annulla</button>
      <button type="button" onclick={confirmConflictSwitch} disabled={conflictBusy}>
        {conflictBusy ? "In corso…" : "Conferma"}
      </button>
    </div>
  </div>
</Modal>

<Modal bind:open={mountModalOpen} title={`Mount — ${remoteName}`}>
  <div class="modal-form">
    <p class="hint">Cartella locale</p>
    <div class="folder-picker">
      <input type="text" bind:value={mountFormPoint} placeholder="Nessuna cartella scelta" readonly />
      <button type="button" onclick={pickMountFolder}>Scegli cartella…</button>
    </div>
    <p class="hint">Cartella su {remoteName}</p>
    <div class="folder-picker">
      <input type="text" value={mountFormRemotePath === "" ? "/ (radice)" : `/${mountFormRemotePath}`} placeholder="" readonly />
      <button type="button" onclick={() => openRemotePicker((p) => (mountFormRemotePath = p))}>Sfoglia…</button>
    </div>
    <label class="checkbox-row">
      <input type="checkbox" bind:checked={mountFormAuto} />
      Monta automaticamente all'avvio dell'app
    </label>
    {#if mountEntry}
      <p class="hint">{mountEntry.mounted ? "Attualmente montato." : "Attualmente non montato."}</p>
    {/if}
    {#if mountError}
      <p class="error">✗ {mountError}</p>
    {/if}
    <div class="row-actions modal-actions">
      {#if mountEntry?.mounted}
        <button type="button" onclick={unmountOnly} disabled={mountBusy}>{mountBusy ? "In corso…" : "Smonta"}</button>
      {:else}
        <button type="button" onclick={confirmMount} disabled={mountBusy || mountFormPoint.trim() === ""}>
          {mountBusy ? "In corso…" : "Monta e apri"}
        </button>
      {/if}
    </div>
  </div>
</Modal>

<Modal bind:open={backupModalOpen} title={`Backup — ${remoteName}`}>
  <div class="modal-form">
    <div class="direction-toggle">
      <label class="direction-option" class:selected={backupFormDirection === "toRemote"}>
        <input type="radio" bind:group={backupFormDirection} value="toRemote" />
        Locale → {remoteName}
      </label>
      <label class="direction-option" class:selected={backupFormDirection === "fromRemote"}>
        <input type="radio" bind:group={backupFormDirection} value="fromRemote" />
        {remoteName} → Locale
      </label>
    </div>
    <p class="hint">Cartella locale</p>
    <div class="folder-picker">
      <input type="text" bind:value={backupFormLocalPath} placeholder="Nessuna cartella scelta" readonly />
      <button type="button" onclick={pickBackupFolder}>Scegli cartella…</button>
    </div>
    <p class="hint">Cartella su {remoteName}</p>
    <div class="folder-picker">
      <input type="text" value={backupFormRemotePath === "" ? "/ (radice)" : `/${backupFormRemotePath}`} placeholder="" readonly />
      <button type="button" onclick={() => openRemotePicker((p) => (backupFormRemotePath = p))}>Sfoglia…</button>
    </div>
    <label class="checkbox-row">
      <input type="checkbox" bind:checked={backupFormAutoEnabled} />
      Esegui automaticamente
    </label>
    {#if backupFormAutoEnabled}
      <label class="interval-row">
        Ogni <input type="number" min="1" bind:value={backupFormAutoInterval} /> minuti
      </label>
    {/if}
    <label class="checkbox-row">
      <input type="checkbox" bind:checked={backupFormPropagateDeletions} onchange={onPropagateDeletionsChange} />
      Propaga cancellazioni
    </label>
    <p class="hint">
      {#if backupFormPropagateDeletions}
        Attivo: i file rimossi dalla sorgente vengono rimossi anche dalla destinazione ad ogni sincronizzazione.
      {:else}
        Disattivo (consigliato): la destinazione accumula solo copie, nulla viene mai cancellato automaticamente
        anche se sparisce dalla sorgente.
      {/if}
    </p>
    {#if backupRunResult}
      {#if backupRunResult.success}
        <p class="ok">✓ eseguito ora</p>
      {:else}
        <p class="error">✗ {backupRunResult.message}</p>
      {/if}
    {/if}
    {#if backupError}
      <p class="error">✗ {backupError}</p>
    {/if}
    <div class="row-actions modal-actions">
      {#if syncJob}
        <button type="button" onclick={runBackupNow} disabled={backupBusy}>{backupBusy ? "In corso…" : "Esegui ora"}</button>
      {/if}
      <button type="button" onclick={saveBackup} disabled={backupBusy || backupFormLocalPath.trim() === ""}>
        {backupBusy ? "Salvataggio…" : "Salva e chiudi"}
      </button>
      {#if backupWasActiveOnOpen}
        <button type="button" onclick={disableBackupAndClose} disabled={backupBusy}>Disabilita</button>
      {/if}
    </div>
  </div>
</Modal>

<Modal bind:open={propagateDeletionsWarningOpen} title="Attenzione">
  <div class="modal-form">
    <p>Stai per attivare "Propaga cancellazioni" per il backup di {remoteName}.</p>
    <p>
      <strong>Se la attivi:</strong> ad ogni sincronizzazione i file rimossi (o non più presenti) nella sorgente
      verranno rimossi anche dalla destinazione — la destinazione diventa uno specchio esatto della sorgente. Se la
      sorgente risultasse vuota o momentaneamente irraggiungibile (es. un disco esterno scollegato), rischi di
      svuotare la destinazione.
    </p>
    <p>
      <strong>Se non la attivi:</strong> i file arrivano e vengono aggiornati nella destinazione, ma nulla viene mai
      cancellato automaticamente, anche se sparisce dalla sorgente — la destinazione può solo accumulare, mai perdere
      dati per una sincronizzazione automatica.
    </p>
    <div class="row-actions modal-actions">
      <button type="button" onclick={() => (propagateDeletionsWarningOpen = false)}>Annulla</button>
      <button type="button" onclick={confirmPropagateDeletions}>Ho capito, attiva</button>
    </div>
  </div>
</Modal>

<Modal bind:open={bisyncModalOpen} title={`Sincronizzazione bidirezionale — ${remoteName}`}>
  <div class="modal-form">
    <p class="hint">Cartella locale</p>
    <div class="folder-picker">
      <input type="text" bind:value={bisyncFormLocalPath} placeholder="Nessuna cartella scelta" readonly />
      <button type="button" onclick={pickBisyncFolder}>Scegli cartella…</button>
    </div>
    <p class="hint">Cartella su {remoteName}</p>
    <div class="folder-picker">
      <input type="text" value={bisyncFormRemotePath === "" ? "/ (radice)" : `/${bisyncFormRemotePath}`} placeholder="" readonly />
      <button type="button" onclick={() => openRemotePicker((p) => (bisyncFormRemotePath = p))}>Sfoglia…</button>
    </div>
    <p class="hint">
      Alla primissima esecuzione, se esiste già un file con lo stesso nome ma contenuto diverso su entrambi i lati,
      viene presa come riferimento la versione modificata più di recente.
    </p>
    <label class="checkbox-row">
      <input type="checkbox" bind:checked={bisyncFormAutoEnabled} />
      Esegui automaticamente
    </label>
    {#if bisyncFormAutoEnabled}
      <label class="interval-row">
        Ogni <input type="number" min="1" bind:value={bisyncFormAutoInterval} /> minuti
      </label>
    {/if}
    {#if bisyncJob?.needsResync}
      <p class="hint">La prossima esecuzione includerà un nuovo resync (baseline).</p>
    {/if}
    {#if bisyncRunResult}
      {#if bisyncRunResult.success && bisyncRunResult.conflictPaths.length === 0}
        <p class="ok">✓ eseguito ora</p>
      {:else if bisyncRunResult.success}
        <div class="conflict-box">
          <strong>⚠ {bisyncRunResult.conflictPaths.length} file in conflitto</strong>
          <p>Nessuna versione è stata persa: entrambe salvate con un suffisso di conflitto.</p>
        </div>
      {:else}
        <p class="error">✗ {bisyncRunResult.message}</p>
      {/if}
    {/if}
    {#if bisyncError}
      <p class="error">✗ {bisyncError}</p>
    {/if}
    <div class="row-actions modal-actions">
      {#if bisyncJob}
        <button type="button" onclick={runBisyncNow} disabled={bisyncBusy}>{bisyncBusy ? "In corso…" : "Esegui ora"}</button>
      {/if}
      <button type="button" onclick={saveBisync} disabled={bisyncBusy || bisyncFormLocalPath.trim() === ""}>
        {bisyncBusy ? "Salvataggio…" : "Salva e chiudi"}
      </button>
      {#if bisyncWasActiveOnOpen}
        <button type="button" onclick={disableBisyncAndClose} disabled={bisyncBusy}>Disabilita</button>
      {/if}
    </div>
  </div>
</Modal>

<RemoteFolderPicker bind:open={remotePickerOpen} {remoteName} onSelect={(path) => remotePickerOnSelect?.(path)} />

<Modal bind:open={historyModalOpen} title={`Cronologia — ${remoteName}`}>
  {#if activeService === null}
    <p class="hint">Nessun servizio attivo per questo remote.</p>
  {:else if activeService === "mount" && mountEntry}
    {#if mountEntry.history.length === 0}
      <p class="hint">Nessun tentativo registrato ancora.</p>
    {:else}
      <ul class="history-list">
        {#each mountEntry.history as entry (entry.whenUnix + entry.action)}
          <li>
            <span class={entry.success ? "ok" : "error"}>{entry.success ? "✓" : "✗"} {entry.action}</span>
            <span class="hint">{formatWhen(entry.whenUnix)}</span>
            {#if !entry.success}<p class="error">{entry.message}</p>{/if}
          </li>
        {/each}
      </ul>
    {/if}
  {:else if activeService === "backup" && syncJob}
    {#if syncJob.history.length === 0}
      <p class="hint">Nessuna esecuzione registrata ancora.</p>
    {:else}
      <ul class="history-list">
        {#each syncJob.history as entry (entry.whenUnix)}
          <li>
            <span class={entry.success ? "ok" : "error"}>{entry.success ? "✓ riuscita" : "✗ fallita"}</span>
            <span class="hint">{formatWhen(entry.whenUnix)}</span>
            {#if !entry.success}<p class="error">{entry.message}</p>{/if}
          </li>
        {/each}
      </ul>
    {/if}
  {:else if activeService === "bisync" && bisyncJob}
    {#if bisyncJob.history.length === 0}
      <p class="hint">Nessuna esecuzione registrata ancora.</p>
    {:else}
      <ul class="history-list">
        {#each bisyncJob.history as entry (entry.whenUnix)}
          <li>
            {#if !entry.success}
              <span class="error">✗ fallita</span>
            {:else if entry.conflictPaths.length > 0}
              <span class="yellow-text">⚠ {entry.conflictPaths.length} conflitti</span>
            {:else}
              <span class="ok">✓ riuscita</span>
            {/if}
            <span class="hint">{formatWhen(entry.whenUnix)}</span>
            {#if !entry.success}<p class="error">{entry.message}</p>{/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</Modal>

<Modal bind:open={deleteRemoteModalOpen} title={`Eliminare ${remoteName}?`}>
  <div class="modal-form">
    <p>Vuoi davvero eliminare questo remote?</p>
    {#if remoteUsage && (remoteUsage.mountName || remoteUsage.backupName || remoteUsage.bisyncName)}
      <div class="conflict-box">
        <strong>Verranno eliminati anche:</strong>
        <ul>
          {#if remoteUsage.mountName}<li>il mount "{remoteUsage.mountName}"</li>{/if}
          {#if remoteUsage.backupName}<li>il backup "{remoteUsage.backupName}"</li>{/if}
          {#if remoteUsage.bisyncName}<li>la sincronizzazione bidirezionale "{remoteUsage.bisyncName}"</li>{/if}
        </ul>
      </div>
    {/if}
    <p class="hint">L'operazione non è reversibile.</p>
    {#if deleteRemoteError}
      <p class="error">✗ {deleteRemoteError}</p>
    {/if}
    <div class="row-actions modal-actions">
      <button type="button" onclick={() => (deleteRemoteModalOpen = false)} disabled={deletingRemote}>Annulla</button>
      <button type="button" onclick={confirmDeleteRemote} disabled={deletingRemote}>
        {deletingRemote ? "Eliminazione…" : "Conferma"}
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

.icon-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 2.6em;
  height: 2.6em;
  font-size: 1rem;
  border-radius: 8px;
  border: 1px solid var(--border-color-subtle);
  background-color: var(--surface-tint);
  color: var(--text-color);
  cursor: pointer;
  text-decoration: none;
  transition: background-color 0.12s ease, border-color 0.12s ease;
}

.icon-button:hover:not(:disabled) {
  filter: brightness(1.15);
}

.icon-button:disabled {
  cursor: default;
  opacity: 0.5;
}

.icon-button.active {
  background-color: var(--accent-bg);
  color: var(--accent);
  border-color: var(--accent);
}

.modal-form {
  display: flex;
  flex-direction: column;
  gap: 0.7em;
}

.modal-actions {
  justify-content: flex-end;
}

.direction-toggle {
  display: flex;
  gap: 0.6em;
  flex-wrap: wrap;
}

.direction-option {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 0.4em;
  padding: 0.4em 0.6em;
  border-radius: 6px;
  border: 1px solid var(--border-color);
  font-size: 0.9em;
}

.direction-option.selected {
  border-color: var(--accent);
  background-color: var(--accent-bg);
}

.direction-option input {
  width: auto;
}

.conflict-box {
  padding: 0.6em 0.8em;
  border-radius: 6px;
  background-color: var(--warning-bg);
  color: var(--warning-text);
  font-size: 0.85em;
}

.conflict-box p {
  margin: 0.3em 0 0;
}

.conflict-box ul {
  margin: 0.3em 0 0;
  padding-left: 1.2em;
}

.yellow-text {
  color: var(--warning-text);
}

.history-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 0.6em;
}

.history-list li {
  display: flex;
  flex-direction: column;
  gap: 0.15em;
  padding-bottom: 0.6em;
  border-bottom: 1px solid var(--border-color-subtle);
}

.history-list li:last-child {
  border-bottom: none;
}
</style>
