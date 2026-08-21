<script lang="ts">
  // Pannello unico per un remote — sostituisce la costellazione di modal
  // separati che c'era prima (mount/backup/bisync/cronologia/cestino×2/
  // avviso propagazione/avviso home×2/conflitto: 11 stati "aperto" diversi,
  // fino a due impilati insieme). Un solo `Modal` aperto alla volta (quello
  // che ospita QUESTO pannello, gestito dal chiamante), quattro schede
  // interne — mai un secondo popup sopra. Vedi l'audit UX del 21/8/2026 per
  // il ragionamento completo dietro questa scelta.
  import { invoke } from "@tauri-apps/api/core";
  import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
  import Icon from "./Icon.svelte";
  import LogView from "./LogView.svelte";
  import TrashView from "./TrashView.svelte";
  import RemoteFolderPicker from "./RemoteFolderPicker.svelte";
  import type { MountEntry, SyncJob, BisyncJob, BisyncRunEntry, TransferEvent } from "$lib/types";
  import { t } from "$lib/i18n";

  let {
    remoteName,
    mountEntry,
    syncJob,
    bisyncJob,
    onRefresh,
  }: {
    remoteName: string;
    mountEntry: MountEntry | null;
    syncJob: SyncJob | null;
    bisyncJob: BisyncJob | null;
    onRefresh?: () => void | Promise<void>;
  } = $props();

  let prefix = $derived(`${remoteName}:`);

  type ServiceKind = "mount" | "backup" | "bisync";

  let activeService = $derived.by<ServiceKind | null>(() => {
    if (mountEntry?.mounted) return "mount";
    if (syncJob?.autoIntervalMinutes !== null && syncJob !== null) return "backup";
    if (bisyncJob?.autoIntervalMinutes !== null && bisyncJob !== null) return "bisync";
    return null;
  });

  // Quale servizio mostrare per primo aprendo il pannello: quello attivo se
  // c'è, altrimenti il primo che ha comunque una configurazione salvata
  // (manuale, mai automatizzata), altrimenti nessuno — mostra il selettore.
  // Un remote può avere più configurazioni salvate contemporaneamente (il
  // backend impedisce solo un secondo mount/backup/bisync DELLO STESSO tipo,
  // non tipi diversi insieme) — "Cambia servizio" nella scheda Configura
  // resta sempre disponibile per raggiungere le altre.
  function initialService(): ServiceKind | null {
    if (activeService) return activeService;
    if (mountEntry) return "mount";
    if (syncJob) return "backup";
    if (bisyncJob) return "bisync";
    return null;
  }

  // `initialService()` valutata una volta sola qui (non dentro `$derived`,
  // dato che gli `$state` sotto vanno inizializzati con IL VALORE di
  // apertura, non tenuti agganciati per sempre a quella funzione).
  const openingService = initialService();
  let selectedService = $state<ServiceKind | null>(openingService);
  type Tab = "configura" | "esegui" | "cronologia" | "cestino";
  let activeTab = $state<Tab>(openingService ? "esegui" : "configura");

  function activateServiceForm(kind: ServiceKind) {
    selectedService = kind;
    if (kind === "mount") initMountForm();
    else if (kind === "backup") initBackupForm();
    else initBisyncForm();
    activeTab = "configura";
  }

  // Al più un servizio automatizzato per remote (vincolo imposto anche dal
  // backend, `ensure_no_other_active_service`): passare a un servizio
  // diverso da quello già attivo richiede prima di disattivare quello
  // vecchio. Prima lo si intercettava PRIMA di far compilare qualunque
  // campo (stesso principio anche qui: un avviso inline, non un secondo
  // modal impilato) invece di lasciare che l'utente scoprisse il rifiuto
  // solo al salvataggio.
  let pendingServiceSwitch = $state<ServiceKind | null>(null);
  let serviceSwitchBusy = $state(false);
  let serviceSwitchError = $state<string | null>(null);

  function requestService(kind: ServiceKind) {
    if (activeService !== null && activeService !== kind) {
      pendingServiceSwitch = kind;
      serviceSwitchError = null;
      return;
    }
    activateServiceForm(kind);
  }

  async function disableActiveService() {
    if (activeService === "mount" && mountEntry) {
      if (mountEntry.mounted) await invoke("unmount_now", { name: mountEntry.name });
      if (mountEntry.autoMount) {
        await invoke("update_mount", {
          oldName: mountEntry.name,
          name: mountEntry.name,
          remote: mountEntry.remote,
          mountPoint: mountEntry.mountPoint,
          autoMount: false,
        });
      }
    } else if (activeService === "backup" && syncJob) {
      await invoke("update_job", {
        oldName: syncJob.name,
        name: syncJob.name,
        source: syncJob.source,
        destination: syncJob.destination,
        autoIntervalMinutes: null,
        propagateDeletions: syncJob.propagateDeletions,
      });
    } else if (activeService === "bisync" && bisyncJob) {
      await invoke("update_bisync_job", { oldName: bisyncJob.name, name: bisyncJob.name, path1: bisyncJob.path1, path2: bisyncJob.path2, autoIntervalMinutes: null });
    }
  }

  async function confirmServiceSwitch() {
    if (!pendingServiceSwitch) return;
    serviceSwitchBusy = true;
    serviceSwitchError = null;
    try {
      await disableActiveService();
      const target = pendingServiceSwitch;
      pendingServiceSwitch = null;
      await onRefresh?.();
      activateServiceForm(target);
    } catch (error) {
      serviceSwitchError = String(error);
    } finally {
      serviceSwitchBusy = false;
    }
  }

  const SERVICE_LABELS: Record<ServiceKind, string> = $derived({
    mount: $t("remoteRow.serviceLabel.mount"),
    backup: $t("remoteRow.serviceLabel.backup"),
    bisync: $t("remoteRow.serviceLabel.bisync"),
  });

  // --- Selettore di cartella remota, condiviso da mount/backup/bisync: al
  // più un modulo di configurazione è visibile alla volta in questo
  // pannello, quindi al più un "Sfoglia…" può essere in corso. ---
  let remotePickerOpen = $state(false);
  let remotePickerOnSelect: ((path: string) => void) | null = $state(null);
  function openRemotePicker(onSelect: (path: string) => void) {
    remotePickerOnSelect = onSelect;
    remotePickerOpen = true;
  }
  function remoteSubPathOf(fs: string): string {
    return fs.startsWith(prefix) ? fs.slice(prefix.length) : "";
  }

  function formatWhen(whenUnix: number): string {
    return new Date(whenUnix * 1000).toLocaleString();
  }

  function formatTransfers(transfers: TransferEvent[]): string {
    const labels: Record<string, string> = {
      transferring: $t("remoteRow.transferWhatTransferring"),
      deleting: $t("remoteRow.transferWhatDeleting"),
      moving: $t("remoteRow.transferWhatMoving"),
      renaming: $t("remoteRow.transferWhatRenaming"),
    };
    return transfers
      .map((event) => {
        const label = labels[event.what] ?? event.what;
        return event.error ? `${label}: ${event.name} — ${event.error}` : `${label}: ${event.name}`;
      })
      .join("\n");
  }

  // ============================= MOUNT =============================
  let mountBusy = $state(false);
  let mountError = $state<string | null>(null);
  let mountFormPoint = $state("");
  let mountFormAuto = $state(false);
  let mountFormRemotePath = $state("");

  function initMountForm() {
    mountFormPoint = mountEntry?.mountPoint ?? "";
    mountFormAuto = mountEntry?.autoMount ?? false;
    mountFormRemotePath = mountEntry ? remoteSubPathOf(mountEntry.remote) : "";
    mountError = null;
    winFspInstallStarted = false;
  }

  async function pickMountFolder() {
    const selected = await openFolderDialog({ directory: true, multiple: false, title: $t("remoteRow.chooseOrCreateFolderDialogTitle") });
    if (typeof selected === "string") mountFormPoint = selected;
  }

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
    } catch (error) {
      mountError = String(error);
    } finally {
      mountBusy = false;
    }
  }

  let mountErrorNeedsWinFsp = $derived(mountError?.includes("WinFsp") ?? false);
  let winFspInstallBusy = $state(false);
  let winFspInstallStarted = $state(false);

  async function installWinFsp() {
    winFspInstallBusy = true;
    try {
      await invoke("download_and_launch_winfsp_installer");
      winFspInstallStarted = true;
    } catch (error) {
      mountError = String(error);
    } finally {
      winFspInstallBusy = false;
    }
  }

  async function unmountOnly() {
    if (!mountEntry) return;
    mountBusy = true;
    mountError = null;
    try {
      await invoke("unmount_now", { name: mountEntry.name });
      await onRefresh?.();
    } catch (error) {
      mountError = String(error);
    } finally {
      mountBusy = false;
    }
  }

  // ============================= BACKUP =============================
  type Direction = "toRemote" | "fromRemote";
  let backupBusy = $state(false);
  let backupError = $state<string | null>(null);
  let backupFormDirection = $state<Direction>("toRemote");
  let backupFormLocalPath = $state("");
  let backupFormAutoEnabled = $state(true);
  let backupFormAutoInterval = $state(15);
  let backupFormPropagateDeletions = $state(false);
  let backupRunResult = $state<{ success: boolean; message: string } | null>(null);
  // Ripiega sull'ultimo tentativo persistito (`syncJob.history[0]`) quando
  // non c'è ancora un risultato fresco in questa sessione — senza questo,
  // riaprendo il pannello su un backup l'ultima volta fallito, la scheda
  // "Esegui e stato" mostrava solo i controlli di routine, senza una parola
  // sul perché, esattamente il problema di "stato onesto" che questo giro
  // di lavoro doveva risolvere anche qui, non solo nella riga collassata.
  let backupDisplayResult = $derived<{ success: boolean; message: string } | null>(
    backupRunResult ?? (syncJob?.history[0] ? { success: syncJob.history[0].success, message: syncJob.history[0].message } : null),
  );
  let backupFormDryRun = $state(false);
  let backupFormRemotePath = $state("");
  let backupPathRootError = $state<string | null>(null);
  // Avvisi che PRIMA erano un secondo Modal impilato sopra quello di
  // configurazione — ora un riquadro inline nella stessa scheda: nessuno
  // sparisce, semplicemente non c'è più un secondo livello di popup.
  let backupPropagateWarningOpen = $state(false);
  let backupHomeWarningPath = $state<string | null>(null);

  function directionOf(job: SyncJob): Direction {
    return job.source.startsWith(prefix) ? "fromRemote" : "toRemote";
  }
  function localPathOf(job: SyncJob): string {
    return directionOf(job) === "toRemote" ? job.source : job.destination;
  }
  function remoteFsOf(job: SyncJob): string {
    return directionOf(job) === "toRemote" ? job.destination : job.source;
  }

  function initBackupForm() {
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
    backupError = null;
    backupRunResult = null;
    backupFormDryRun = false;
    backupPathRootError = null;
    backupPropagateWarningOpen = false;
    backupHomeWarningPath = null;
  }

  async function pickBackupFolder() {
    const selected = await openFolderDialog({ directory: true, multiple: false, title: $t("remoteRow.chooseFolderDialogTitle") });
    if (typeof selected !== "string") return;
    backupPathRootError = null;
    const risk = await invoke<string | null>("check_dangerous_path", { path: selected });
    if (risk === "root") {
      backupPathRootError = $t("remoteRow.dangerousPathRootError");
      return;
    }
    if (risk === "home") {
      backupHomeWarningPath = selected;
      return;
    }
    backupFormLocalPath = selected;
  }

  function confirmBackupHomePath() {
    if (backupHomeWarningPath) backupFormLocalPath = backupHomeWarningPath;
    backupHomeWarningPath = null;
  }

  function onPropagateDeletionsChange() {
    if (backupFormPropagateDeletions) {
      backupFormPropagateDeletions = false;
      backupPropagateWarningOpen = true;
    }
  }

  function confirmPropagateDeletions() {
    backupFormPropagateDeletions = true;
    backupPropagateWarningOpen = false;
  }

  async function persistBackup() {
    const remoteFs = `${prefix}${backupFormRemotePath}`;
    const source = backupFormDirection === "toRemote" ? backupFormLocalPath.trim() : remoteFs;
    const destination = backupFormDirection === "toRemote" ? remoteFs : backupFormLocalPath.trim();
    const autoIntervalMinutes = backupFormAutoEnabled ? backupFormAutoInterval : null;
    const propagateDeletions = backupFormPropagateDeletions;
    if (syncJob) {
      await invoke("update_job", { oldName: syncJob.name, name: remoteName, source, destination, autoIntervalMinutes, propagateDeletions });
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
      if (backupFormDryRun) {
        await invoke("dry_run_job", { name: remoteName });
      } else {
        await invoke("run_job", { name: remoteName });
        backupRunResult = { success: true, message: "" };
      }
    } catch (error) {
      if (backupFormDryRun) {
        backupError = String(error);
      } else {
        backupRunResult = { success: false, message: String(error) };
      }
    } finally {
      backupBusy = false;
      await onRefresh?.();
    }
  }

  // ============================= BISYNC =============================
  let bisyncBusy = $state(false);
  let bisyncError = $state<string | null>(null);
  let bisyncFormLocalPath = $state("");
  let bisyncFormAutoEnabled = $state(true);
  let bisyncFormAutoInterval = $state(15);
  let bisyncRunResult = $state<BisyncRunEntry | null>(null);
  // Stesso principio di `backupDisplayResult` qui sopra: senza questo, un
  // bisync bloccato da "too many deletes" tornava a mostrare solo
  // "Esegui ora" appena si riapriva il pannello, nascondendo il blocco e il
  // pulsante "Forza comunque" finché non si andava a cercarli in Cronologia.
  let bisyncDisplayResult = $derived<BisyncRunEntry | null>(bisyncRunResult ?? bisyncJob?.history[0] ?? null);
  let bisyncFormRemotePath = $state("");
  let bisyncFormDryRun = $state(false);
  let expandedHistoryEntryKey = $state<number | null>(null);
  let bisyncPathRootError = $state<string | null>(null);
  let bisyncHomeWarningPath = $state<string | null>(null);

  function bisyncLocalPathOf(job: BisyncJob): string {
    return job.path1.startsWith(prefix) ? job.path2 : job.path1;
  }
  function bisyncRemoteFsOf(job: BisyncJob): string {
    return job.path1.startsWith(prefix) ? job.path1 : job.path2;
  }

  function initBisyncForm() {
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
    bisyncError = null;
    bisyncRunResult = null;
    bisyncFormDryRun = false;
    bisyncPathRootError = null;
    bisyncHomeWarningPath = null;
  }

  async function pickBisyncFolder() {
    const selected = await openFolderDialog({ directory: true, multiple: false, title: $t("remoteRow.chooseFolderDialogTitle") });
    if (typeof selected !== "string") return;
    bisyncPathRootError = null;
    const risk = await invoke<string | null>("check_dangerous_path", { path: selected });
    if (risk === "root") {
      bisyncPathRootError = $t("remoteRow.dangerousPathRootError");
      return;
    }
    if (risk === "home") {
      bisyncHomeWarningPath = selected;
      return;
    }
    bisyncFormLocalPath = selected;
  }

  function confirmBisyncHomePath() {
    if (bisyncHomeWarningPath) bisyncFormLocalPath = bisyncHomeWarningPath;
    bisyncHomeWarningPath = null;
  }

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
      if (bisyncFormDryRun) {
        await invoke("dry_run_bisync_job", { name: remoteName });
      } else {
        bisyncRunResult = await invoke<BisyncRunEntry>("run_bisync_job", { name: remoteName });
      }
    } catch (error) {
      bisyncError = String(error);
    } finally {
      bisyncBusy = false;
      await onRefresh?.();
    }
  }

  async function forceBisyncNow() {
    bisyncBusy = true;
    bisyncError = null;
    try {
      bisyncRunResult = await invoke<BisyncRunEntry>("run_bisync_job_forced", { name: remoteName });
    } catch (error) {
      bisyncError = String(error);
    } finally {
      bisyncBusy = false;
      await onRefresh?.();
    }
  }

  // Inizializza subito il servizio mostrato all'apertura (se già noto).
  if (openingService === "mount") initMountForm();
  else if (openingService === "backup") initBackupForm();
  else if (openingService === "bisync") initBisyncForm();
</script>

<div class="panel">
  <div class="tabs">
    <button type="button" class="tab" class:active={activeTab === "configura"} onclick={() => (activeTab = "configura")}>
      {$t("remotePanel.tabConfigure")}
    </button>
    {#if selectedService}
      <button type="button" class="tab" class:active={activeTab === "esegui"} onclick={() => (activeTab = "esegui")}>
        {$t("remotePanel.tabRun")}
      </button>
      <button type="button" class="tab" class:active={activeTab === "cronologia"} onclick={() => (activeTab = "cronologia")}>
        {$t("remotePanel.tabHistory")}
      </button>
      {#if selectedService !== "mount"}
        <button type="button" class="tab" class:active={activeTab === "cestino"} onclick={() => (activeTab = "cestino")}>
          {$t("trash.title")}
        </button>
      {/if}
    {/if}
  </div>

  <div class="tab-content">
    {#if activeTab === "configura"}
      {#if selectedService === null}
        <p class="hint">{$t("remotePanel.pickServiceHint")}</p>
        <div class="service-picker">
          <button type="button" class="service-card" onclick={() => requestService("mount")}>
            <Icon kind="mount" />
            <div>
              <strong>{$t("remoteRow.service.mount")}</strong>
              <p class="hint">{$t("remoteRow.mountAction")}</p>
            </div>
          </button>
          <button type="button" class="service-card" onclick={() => requestService("backup")}>
            <Icon kind="backup" />
            <div>
              <strong>{$t("remoteRow.service.backup")}</strong>
              <p class="hint">{$t("remoteRow.backupAction")}</p>
            </div>
          </button>
          <button type="button" class="service-card" onclick={() => requestService("bisync")}>
            <Icon kind="bisync" />
            <div>
              <strong>{$t("remoteRow.service.bisync")}</strong>
              <p class="hint">{$t("remoteRow.bisyncAction")}</p>
            </div>
          </button>
        </div>
        {#if pendingServiceSwitch}
          <div class="inline-warning">
            <p>
              {$t("remotePanel.serviceSwitchWarning", {
                values: { active: activeService ? SERVICE_LABELS[activeService] : "", target: SERVICE_LABELS[pendingServiceSwitch] },
              })}
            </p>
            {#if serviceSwitchError}
              <p>✗ {serviceSwitchError}</p>
            {/if}
            <div class="row-actions modal-actions">
              <button type="button" onclick={() => (pendingServiceSwitch = null)} disabled={serviceSwitchBusy}>{$t("common.cancel")}</button>
              <button type="button" onclick={confirmServiceSwitch} disabled={serviceSwitchBusy}>
                {serviceSwitchBusy ? $t("common.inProgress") : $t("common.confirm")}
              </button>
            </div>
          </div>
        {/if}
      {:else}
        <button type="button" class="change-service-link" onclick={() => (selectedService = null)}>
          {$t("remotePanel.changeService", { values: { current: SERVICE_LABELS[selectedService] } })}
        </button>

        {#if selectedService === "mount"}
          <div class="modal-form">
            <p class="hint">{$t("remoteRow.localFolder")}</p>
            <div class="folder-picker">
              <input type="text" bind:value={mountFormPoint} placeholder={$t("remoteRow.noFolderChosen")} readonly />
              <button type="button" onclick={pickMountFolder}>{$t("common.chooseFolder")}</button>
            </div>
            <p class="hint">{$t("folderPicker.title", { values: { remote: remoteName } })}</p>
            <div class="folder-picker">
              <input type="text" value={mountFormRemotePath === "" ? $t("remoteRow.rootPath") : `/${mountFormRemotePath}`} placeholder="" readonly />
              <button type="button" onclick={() => openRemotePicker((p) => (mountFormRemotePath = p))}>{$t("common.browse")}</button>
            </div>
            <label class="checkbox-row">
              <input type="checkbox" bind:checked={mountFormAuto} />
              {$t("remoteRow.autoMountAtStartup")}
            </label>
            {#if mountEntry}
              <p class="hint">{mountEntry.mounted ? $t("remoteRow.currentlyMounted") : $t("remoteRow.currentlyNotMounted")}</p>
            {/if}
            {#if mountError}
              <LogView text={mountError} />
            {/if}
            {#if mountErrorNeedsWinFsp}
              {#if winFspInstallStarted}
                <p class="hint">{$t("remoteRow.winFspInstallerStarted")}</p>
              {:else}
                <button type="button" onclick={installWinFsp} disabled={winFspInstallBusy}>
                  {winFspInstallBusy ? $t("remoteRow.downloadingWinFsp") : $t("remoteRow.installWinFsp")}
                </button>
              {/if}
            {/if}
            <div class="row-actions modal-actions">
              {#if mountEntry?.mounted}
                <button type="button" class="btn-danger" onclick={unmountOnly} disabled={mountBusy}>
                  {mountBusy ? $t("common.inProgress") : $t("remoteRow.unmount")}
                </button>
              {:else}
                <button type="button" class="btn-primary" onclick={confirmMount} disabled={mountBusy || mountFormPoint.trim() === ""}>
                  {mountBusy ? $t("common.inProgress") : $t("remoteRow.mountAndOpen")}
                </button>
              {/if}
            </div>
          </div>
        {:else if selectedService === "backup"}
          <div class="modal-form">
            <div class="direction-toggle">
              <label class="direction-option" class:selected={backupFormDirection === "toRemote"}>
                <input type="radio" bind:group={backupFormDirection} value="toRemote" />
                {$t("remoteRow.localToRemote", { values: { remote: remoteName } })}
              </label>
              <label class="direction-option" class:selected={backupFormDirection === "fromRemote"}>
                <input type="radio" bind:group={backupFormDirection} value="fromRemote" />
                {$t("remoteRow.remoteToLocal", { values: { remote: remoteName } })}
              </label>
            </div>
            <p class="hint">{$t("remoteRow.localFolder")}</p>
            <div class="folder-picker">
              <input type="text" bind:value={backupFormLocalPath} placeholder={$t("remoteRow.noFolderChosen")} readonly />
              <button type="button" onclick={pickBackupFolder}>{$t("common.chooseFolder")}</button>
            </div>
            {#if backupPathRootError}
              <p class="error">✗ {backupPathRootError}</p>
            {/if}
            {#if backupHomeWarningPath}
              <div class="inline-warning">
                <p>{$t("remoteRow.dangerousPathHomeWarning", { values: { path: backupHomeWarningPath } })}</p>
                <div class="row-actions modal-actions">
                  <button type="button" onclick={() => (backupHomeWarningPath = null)}>{$t("common.cancel")}</button>
                  <button type="button" onclick={confirmBackupHomePath}>{$t("remoteRow.dangerousPathHomeConfirm")}</button>
                </div>
              </div>
            {/if}
            <p class="hint">{$t("folderPicker.title", { values: { remote: remoteName } })}</p>
            <div class="folder-picker">
              <input
                type="text"
                value={backupFormRemotePath === "" ? $t("remoteRow.rootPath") : `/${backupFormRemotePath}`}
                placeholder=""
                readonly
              />
              <button type="button" onclick={() => openRemotePicker((p) => (backupFormRemotePath = p))}>{$t("common.browse")}</button>
            </div>
            <label class="checkbox-row">
              <input type="checkbox" bind:checked={backupFormAutoEnabled} />
              {$t("remoteRow.runAutomatically")}
            </label>
            {#if backupFormAutoEnabled}
              <label class="interval-row">
                {$t("remoteRow.intervalPrefix")} <input type="number" min="1" bind:value={backupFormAutoInterval} /> {$t("remoteRow.intervalSuffix")}
              </label>
            {/if}
            <label class="checkbox-row">
              <input type="checkbox" bind:checked={backupFormPropagateDeletions} onchange={onPropagateDeletionsChange} />
              {$t("remoteRow.propagateDeletions")}
            </label>
            {#if backupPropagateWarningOpen}
              <div class="inline-warning">
                <p>{$t("remoteRow.propagateDeletionsWarningIntro", { values: { remote: remoteName } })}</p>
                <p><strong>{$t("remoteRow.propagateDeletionsWarningOnTitle")}</strong> {$t("remoteRow.propagateDeletionsWarningOnBody")}</p>
                <p><strong>{$t("remoteRow.propagateDeletionsWarningOffTitle")}</strong> {$t("remoteRow.propagateDeletionsWarningOffBody")}</p>
                <div class="row-actions modal-actions">
                  <button type="button" onclick={() => (backupPropagateWarningOpen = false)}>{$t("common.cancel")}</button>
                  <button type="button" onclick={confirmPropagateDeletions}>{$t("remoteRow.understoodEnable")}</button>
                </div>
              </div>
            {:else}
              <p class="hint">
                {backupFormPropagateDeletions ? $t("remoteRow.propagateDeletionsOnHint") : $t("remoteRow.propagateDeletionsOffHint")}
              </p>
            {/if}
            {#if backupError}
              <LogView text={backupError} />
            {/if}
            <div class="row-actions modal-actions">
              <button type="button" class="btn-primary" onclick={saveBackup} disabled={backupBusy || backupFormLocalPath.trim() === ""}>
                {backupBusy ? $t("remoteRow.saving") : $t("remoteRow.saveChanges")}
              </button>
            </div>
          </div>
        {:else}
          <div class="modal-form">
            <p class="hint">{$t("remoteRow.localFolder")}</p>
            <div class="folder-picker">
              <input type="text" bind:value={bisyncFormLocalPath} placeholder={$t("remoteRow.noFolderChosen")} readonly />
              <button type="button" onclick={pickBisyncFolder}>{$t("common.chooseFolder")}</button>
            </div>
            {#if bisyncPathRootError}
              <p class="error">✗ {bisyncPathRootError}</p>
            {/if}
            {#if bisyncHomeWarningPath}
              <div class="inline-warning">
                <p>{$t("remoteRow.dangerousPathHomeWarning", { values: { path: bisyncHomeWarningPath } })}</p>
                <div class="row-actions modal-actions">
                  <button type="button" onclick={() => (bisyncHomeWarningPath = null)}>{$t("common.cancel")}</button>
                  <button type="button" onclick={confirmBisyncHomePath}>{$t("remoteRow.dangerousPathHomeConfirm")}</button>
                </div>
              </div>
            {/if}
            <p class="hint">{$t("folderPicker.title", { values: { remote: remoteName } })}</p>
            <div class="folder-picker">
              <input
                type="text"
                value={bisyncFormRemotePath === "" ? $t("remoteRow.rootPath") : `/${bisyncFormRemotePath}`}
                placeholder=""
                readonly
              />
              <button type="button" onclick={() => openRemotePicker((p) => (bisyncFormRemotePath = p))}>{$t("common.browse")}</button>
            </div>
            <p class="hint">{$t("remoteRow.bisyncFirstRunHint")}</p>
            <label class="checkbox-row">
              <input type="checkbox" bind:checked={bisyncFormAutoEnabled} />
              {$t("remoteRow.runAutomatically")}
            </label>
            {#if bisyncFormAutoEnabled}
              <label class="interval-row">
                {$t("remoteRow.intervalPrefix")} <input type="number" min="1" bind:value={bisyncFormAutoInterval} /> {$t("remoteRow.intervalSuffix")}
              </label>
            {/if}
            {#if bisyncJob?.needsResync}
              <p class="hint">{$t("remoteRow.needsResyncHint")}</p>
            {/if}
            {#if bisyncError}
              <LogView text={bisyncError} />
            {/if}
            <div class="row-actions modal-actions">
              <button type="button" class="btn-primary" onclick={saveBisync} disabled={bisyncBusy || bisyncFormLocalPath.trim() === ""}>
                {bisyncBusy ? $t("remoteRow.saving") : $t("remoteRow.saveChanges")}
              </button>
            </div>
          </div>
        {/if}
      {/if}
    {/if}

    {#if activeTab === "esegui" && selectedService}
      <div class="modal-form">
        {#if selectedService === "backup"}
          <label class="checkbox-row">
            <input type="checkbox" bind:checked={backupFormDryRun} />
            {$t("remoteRow.dryRunCheckbox")}
          </label>
          {#if backupDisplayResult}
            {#if backupDisplayResult.success}
              <p class="ok">✓ {backupRunResult ? $t("remoteRow.executedNow") : $t("remoteRow.lastRunSucceeded")}</p>
            {:else}
              <LogView text={backupDisplayResult.message} />
            {/if}
          {/if}
          {#if syncJob?.lastDryRun && !backupRunResult && syncJob.lastDryRun.whenUnix > (syncJob.history[0]?.whenUnix ?? 0)}
            {@const report = syncJob.lastDryRun}
            {@const localIsSource = directionOf(syncJob) === "toRemote"}
            <div class="conflict-box">
              <strong>{$t("remoteRow.dryRunResultTitle", { values: { when: formatWhen(report.whenUnix) } })}</strong>
              <p>{$t("remoteRow.dryRunLocalTotal", { values: { count: localIsSource ? report.sourceTotalFiles : report.destinationTotalFiles } })}</p>
              <p>{$t("remoteRow.dryRunRemoteTotal", { values: { count: localIsSource ? report.destinationTotalFiles : report.sourceTotalFiles } })}</p>
              <p>{$t("remoteRow.dryRunWouldTransfer", { values: { count: report.wouldTransfer } })}</p>
              <p>
                {#if report.wouldDelete > 0}
                  ⚠ {$t("remoteRow.dryRunWouldDelete", { values: { count: report.wouldDelete } })}
                {:else}
                  {$t("remoteRow.dryRunNoDeletes")}
                {/if}
              </p>
            </div>
          {/if}
          <div class="row-actions modal-actions">
            <button type="button" class="btn-primary" onclick={runBackupNow} disabled={backupBusy || backupFormLocalPath.trim() === ""}>
              {backupBusy
                ? backupFormDryRun
                  ? $t("remoteRow.dryRunInProgress")
                  : $t("common.inProgress")
                : backupFormDryRun
                  ? $t("remoteRow.dryRunButton")
                  : $t("remoteRow.runNow")}
            </button>
          </div>
        {:else if selectedService === "bisync"}
          <label class="checkbox-row">
            <input type="checkbox" bind:checked={bisyncFormDryRun} />
            {$t("remoteRow.dryRunCheckbox")}
          </label>
          {#if bisyncJob?.lastDryRun && !bisyncRunResult && bisyncJob.lastDryRun.whenUnix > (bisyncJob.history[0]?.whenUnix ?? 0)}
            {@const report = bisyncJob.lastDryRun}
            <div class="conflict-box">
              <strong>{$t("remoteRow.dryRunResultTitle", { values: { when: formatWhen(report.whenUnix) } })}</strong>
              <p>{$t("remoteRow.dryRunLocalTotal", { values: { count: report.path1TotalFiles } })}</p>
              <p>{$t("remoteRow.dryRunRemoteTotal", { values: { count: report.path2TotalFiles } })}</p>
              <p>{$t("remoteRow.dryRunWouldTransfer", { values: { count: report.wouldTransfer } })}</p>
              <p>
                {#if report.wouldDelete > 0}
                  ⚠ {$t("remoteRow.dryRunWouldDelete", { values: { count: report.wouldDelete } })}
                {:else}
                  {$t("remoteRow.dryRunNoDeletes")}
                {/if}
              </p>
            </div>
            <LogView text={report.log} />
          {/if}
          {#if bisyncDisplayResult}
            {#if bisyncDisplayResult.success && bisyncDisplayResult.conflictPaths.length === 0}
              <p class="ok">✓ {bisyncRunResult ? $t("remoteRow.executedNow") : $t("remoteRow.lastRunSucceeded")}</p>
            {:else if bisyncDisplayResult.success}
              <div class="conflict-box">
                <strong>⚠ {$t("remoteRow.conflictFilesCount", { values: { count: bisyncDisplayResult.conflictPaths.length } })}</strong>
                <p>{$t("remoteRow.noVersionLost")}</p>
              </div>
            {:else}
              <LogView text={bisyncDisplayResult.log || bisyncDisplayResult.message} />
              {#if bisyncDisplayResult.needsForce}
                <div class="inline-warning">
                  <p>{$t("remoteRow.forceBisyncWarningIntro")}</p>
                  <p>{$t("remoteRow.forceBisyncWarningHint")}</p>
                  <div class="row-actions modal-actions">
                    <button type="button" class="btn-danger" onclick={forceBisyncNow} disabled={bisyncBusy}>
                      {bisyncBusy ? $t("common.inProgress") : $t("remoteRow.runWithForce")}
                    </button>
                  </div>
                </div>
              {/if}
            {/if}
          {/if}
          <div class="row-actions modal-actions">
            <button type="button" class="btn-primary" onclick={runBisyncNow} disabled={bisyncBusy || bisyncFormLocalPath.trim() === ""}>
              {bisyncBusy
                ? bisyncFormDryRun
                  ? $t("remoteRow.dryRunInProgress")
                  : $t("common.inProgress")
                : bisyncFormDryRun
                  ? $t("remoteRow.dryRunButton")
                  : $t("remoteRow.runNow")}
            </button>
          </div>
        {:else if selectedService === "mount"}
          <p class="hint">{mountEntry?.mounted ? $t("remoteRow.currentlyMounted") : $t("remoteRow.currentlyNotMounted")}</p>
          <div class="row-actions modal-actions">
            {#if mountEntry?.mounted}
              <button type="button" class="btn-danger" onclick={unmountOnly} disabled={mountBusy}>
                {mountBusy ? $t("common.inProgress") : $t("remoteRow.unmount")}
              </button>
            {:else}
              <button type="button" class="btn-primary" onclick={confirmMount} disabled={mountBusy || mountFormPoint.trim() === ""}>
                {mountBusy ? $t("common.inProgress") : $t("remoteRow.mountAndOpen")}
              </button>
            {/if}
          </div>
        {/if}
      </div>
    {/if}

    {#if activeTab === "cronologia" && selectedService}
      {#if selectedService === "mount" && mountEntry}
        {#if mountEntry.history.length === 0}
          <p class="hint">{$t("remoteRow.noMountAttempts")}</p>
        {:else}
          <ul class="history-list">
            {#each mountEntry.history as entry (entry.whenUnix + entry.action)}
              <li>
                <span class={entry.success ? "ok" : "error"}>{entry.success ? "✓" : "✗"} {entry.action}</span>
                <span class="hint">{formatWhen(entry.whenUnix)}</span>
                {#if !entry.success}<LogView text={entry.message} />{/if}
              </li>
            {/each}
          </ul>
        {/if}
      {:else if selectedService === "backup" && syncJob}
        {#if syncJob.history.length === 0}
          <p class="hint">{$t("remoteRow.noRunsYet")}</p>
        {:else}
          <ul class="history-list">
            {#each syncJob.history as entry (entry.whenUnix)}
              <li>
                <span class={entry.success ? "ok" : "error"}>{entry.success ? `✓ ${$t("remoteRow.succeeded")}` : `✗ ${$t("remoteRow.failed")}`}</span>
                <span class="hint">{formatWhen(entry.whenUnix)}</span>
                {#if !entry.success}<LogView text={entry.message} />{/if}
                {#if entry.transfers.length > 0}
                  <button type="button" onclick={() => (expandedHistoryEntryKey = expandedHistoryEntryKey === entry.whenUnix ? null : entry.whenUnix)}>
                    {expandedHistoryEntryKey === entry.whenUnix
                      ? $t("remoteRow.hideDetailedLog")
                      : $t("remoteRow.showTransfersCount", { values: { count: entry.transfers.length } })}
                  </button>
                  {#if expandedHistoryEntryKey === entry.whenUnix}
                    <LogView text={formatTransfers(entry.transfers)} />
                  {/if}
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      {:else if selectedService === "bisync" && bisyncJob}
        {#if bisyncJob.history.length === 0}
          <p class="hint">{$t("remoteRow.noRunsYet")}</p>
        {:else}
          <ul class="history-list">
            {#each bisyncJob.history as entry, index (entry.whenUnix)}
              <li>
                {#if !entry.success}
                  <span class="error">✗ {$t("remoteRow.failed")}</span>
                {:else if entry.conflictPaths.length > 0}
                  <span class="yellow-text">⚠ {$t("remoteRow.conflictsCount", { values: { count: entry.conflictPaths.length } })}</span>
                {:else}
                  <span class="ok">✓ {$t("remoteRow.succeeded")}</span>
                {/if}
                <span class="hint">{formatWhen(entry.whenUnix)}</span>
                {#if !entry.success}
                  <LogView text={entry.log || entry.message} />
                  {#if entry.needsForce && index === 0}
                    <div class="inline-warning">
                      <p>{$t("remoteRow.forceBisyncWarningIntro")}</p>
                      <p>{$t("remoteRow.forceBisyncWarningHint")}</p>
                      <div class="row-actions modal-actions">
                        <button type="button" class="btn-danger" onclick={forceBisyncNow} disabled={bisyncBusy}>
                          {bisyncBusy ? $t("common.inProgress") : $t("remoteRow.runWithForce")}
                        </button>
                      </div>
                    </div>
                  {/if}
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    {/if}

    {#if activeTab === "cestino" && selectedService === "backup" && syncJob}
      <TrashView sides={[syncJob.destination]} />
    {:else if activeTab === "cestino" && selectedService === "bisync" && bisyncJob}
      <TrashView sides={[bisyncJob.path1, bisyncJob.path2]} />
    {/if}
  </div>
</div>

<RemoteFolderPicker bind:open={remotePickerOpen} {remoteName} onSelect={(path) => remotePickerOnSelect?.(path)} />

<style>
.panel {
  display: flex;
  flex-direction: column;
  gap: 1em;
}

.tabs {
  display: flex;
  gap: 1.4em;
  border-bottom: 1px solid var(--border-color-subtle);
}

.tab {
  background: none;
  border: none;
  box-shadow: none;
  padding: 0.7em 0.1em;
  border-radius: 0;
  border-bottom: 2.5px solid transparent;
  color: var(--text-muted);
  font-weight: 600;
}

.tab.active {
  color: var(--accent);
  border-bottom-color: var(--accent);
}

.tab-content {
  min-height: 12em;
}

.service-picker {
  display: flex;
  flex-direction: column;
  gap: 0.7em;
  margin-top: 0.8em;
}

.service-card {
  display: flex;
  align-items: center;
  gap: 0.9em;
  text-align: left;
  padding: 0.9em 1.1em;
}

.service-card p {
  margin: 0.15em 0 0;
}

.change-service-link {
  background: none;
  border: none;
  box-shadow: none;
  padding: 0;
  color: var(--accent);
  font-weight: 500;
  font-size: 0.85em;
  margin-bottom: 0.9em;
}

.inline-warning {
  padding: 0.8em 1em;
  border-radius: 8px;
  background-color: var(--warning-bg);
  color: var(--warning-text);
  font-size: 0.9em;
}

.inline-warning :global(p) {
  margin: 0 0 0.5em;
}

.inline-warning .modal-actions {
  margin-top: 0.3em;
}
</style>
