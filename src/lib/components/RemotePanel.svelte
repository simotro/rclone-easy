<script lang="ts">
  // Pannello unico per un remote — sostituisce la costellazione di modal
  // separati che c'era prima (mount/backup/bisync/cronologia/cestino×2/
  // avviso propagazione/avviso home×2/conflitto: 11 stati "aperto" diversi,
  // fino a due impilati insieme). Vive in una pagina propria
  // (src/routes/remote/[name]/+page.svelte), non più dentro un Modal
  // (Simone, 22/8/2026: un contenuto così variabile per altezza/larghezza
  // — form di configurazione, elenchi di cronologia lunghi — dentro una
  // scatola a dimensione fissa produceva salti di dimensione e contenuto
  // tagliato ai bordi, due giri di bug diversi sullo stesso sintomo). Vedi
  // l'audit UX del 21/8/2026 per il ragionamento originale dietro le
  // quattro schede interne.
  import { invoke } from "@tauri-apps/api/core";
  import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
  import { untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import Icon from "./Icon.svelte";
  import LogView from "./LogView.svelte";
  import TrashView from "./TrashView.svelte";
  import ReviewView from "./ReviewView.svelte";
  import RemoteFolderPicker from "./RemoteFolderPicker.svelte";
  import DuplicateGroupModal from "./DuplicateGroupModal.svelte";
  import type { MountEntry, SyncJob, BisyncJob, TransferEvent, TrashEntry, ReviewEntry } from "$lib/types";
  import { t } from "$lib/i18n";

  let {
    remoteName,
    mountEntry,
    syncJob,
    bisyncJob,
    initialTab,
    onRefresh,
  }: {
    remoteName: string;
    mountEntry: MountEntry | null;
    syncJob: SyncJob | null;
    bisyncJob: BisyncJob | null;
    // Dalla tray, una voce di avviso porta direttamente alla Cronologia
    // (dove si vede cosa è fallito) invece che alla scheda Configura di
    // default — vedi src/routes/remote/[name]/+page.svelte.
    initialTab?: "configura" | "cronologia" | "cestino";
    onRefresh?: () => void | Promise<void>;
  } = $props();

  let prefix = $derived(`${remoteName}:`);

  type ServiceKind = "mount" | "backup" | "bisync";

  // Un mount conta come "attivo" anche solo con l'auto-mount all'avvio
  // abilitato, non solo se montato adesso — deve rispecchiare esattamente
  // `active_service_for_remote` nel backend (activity.rs), l'unica autorità
  // reale su cosa il backend rifiuterà di sovrapporre. Prima qui si guardava
  // solo `mounted`: passando ad es. da un mount con auto-mount ma smontato
  // al momento a un backup/bisync, non compariva alcun avviso qui, ma il
  // salvataggio veniva comunque rifiutato dal backend un attimo dopo,
  // incoerenza notata da Simone il 22/8/2026.
  let activeService = $derived.by<ServiceKind | null>(() => {
    if (mountEntry?.mounted || mountEntry?.autoMount) return "mount";
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
  type Tab = "configura" | "cronologia" | "revisione" | "cestino";
  let activeTab = $state<Tab>(untrack(() => initialTab ?? "configura"));

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

  // Cliccare la tab del servizio già selezionato non deve fare nulla — in
  // particolare non deve riportare su "Configura" chi sta guardando "Esegui
  // e stato" o "Cronologia" di quello stesso servizio.
  function selectService(kind: ServiceKind) {
    if (kind === selectedService) return;
    requestService(kind);
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
  let backupDryRunBusy = $state(false);
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

  // "Esegui ora" vero e proprio è passato alla riga del remote in
  // RemoteRow.svelte (Simone, 22/8/2026): questo pannello si occupa solo di
  // provarne l'effetto prima di salvare, non di lanciarlo per davvero.
  async function tryDryRunBackup() {
    if (backupFormLocalPath.trim() === "") return;
    backupDryRunBusy = true;
    backupError = null;
    try {
      await persistBackup();
      await invoke("dry_run_job", { name: remoteName });
    } catch (error) {
      backupError = String(error);
    } finally {
      backupDryRunBusy = false;
      await onRefresh?.();
    }
  }

  // ============================= BISYNC =============================
  let bisyncBusy = $state(false);
  let bisyncError = $state<string | null>(null);
  let bisyncFormLocalPath = $state("");
  let bisyncFormAutoEnabled = $state(true);
  let bisyncFormAutoInterval = $state(15);
  let bisyncDryRunBusy = $state(false);
  let bisyncFormRemotePath = $state("");
  let expandedHistoryEntryKey = $state<number | null>(null);
  let bisyncPathRootError = $state<string | null>(null);
  let bisyncHomeWarningPath = $state<string | null>(null);
  // "Esamina" su un nome duplicato (vedi duplicates.rs) — un solo modal,
  // il nome esaminato è tenuto separato da "aperto" così passare da un
  // nome all'altro non richiede prima chiuderlo e riaprirlo.
  let examiningDuplicateName = $state<string | null>(null);
  let duplicateModalOpen = $state(false);
  // Nomi già lavorati (spostati per revisione o eliminati) in questa
  // sessione — non tolti dall'elenco (l'utente vuole ancora vedere quanti
  // ne restano), solo marcati con una spunta. Non persistito: al prossimo
  // giro di bisync la voce sparisce da sé dall'elenco se davvero risolta.
  // `SvelteSet`, non un `Set` semplice dentro `$state`: le mutazioni come
  // `.add()` su un `Set` nativo non passano dai trap del proxy di `$state`
  // (che intercetta assegnazioni di proprietà, non chiamate di metodo sugli
  // slot interni di Set/Map) — la spunta comparirebbe solo al prossimo giro
  // di rendering innescato da qualcos'altro (es. cambiando tab), non subito.
  let resolvedDuplicateNames = new SvelteSet<string>();

  function examineDuplicate(name: string) {
    examiningDuplicateName = name;
    duplicateModalOpen = true;
  }

  function handleDuplicateResolved(name: string) {
    resolvedDuplicateNames.add(name);
    loadExtraCounts();
  }

  // Conteggi per mostrare le tab "Attesa revisione"/"Cestino" solo quando
  // contengono davvero qualcosa (altrimenti una tab vuota non serve a
  // nessuno) — ricaricati ad ogni cambio di servizio e, tramite le stesse
  // prop `syncJob`/`bisyncJob` ricaricate ogni 10s da +page.svelte, anche
  // periodicamente insieme al resto del pannello.
  let trashCount = $state(0);
  let reviewCount = $state(0);

  async function loadExtraCounts() {
    if (selectedService === "backup" && syncJob) {
      trashCount = (await invoke<TrashEntry[]>("list_trash", { dest: syncJob.destination })).length;
      reviewCount = 0;
    } else if (selectedService === "bisync" && bisyncJob) {
      const [trash1, trash2, review] = await Promise.all([
        invoke<TrashEntry[]>("list_trash", { dest: bisyncJob.path1 }),
        invoke<TrashEntry[]>("list_trash", { dest: bisyncJob.path2 }),
        invoke<ReviewEntry[]>("list_review_entries", { path1: bisyncJob.path1, path2: bisyncJob.path2 }),
      ]);
      trashCount = trash1.length + trash2.length;
      reviewCount = review.length;
    } else {
      trashCount = 0;
      reviewCount = 0;
    }
  }

  $effect(() => {
    loadExtraCounts();
  });

  // Se la tab attiva smette di avere contenuto (es. ultimo file eliminato
  // definitivamente dalla scheda stessa) non deve restare "appesa" su una
  // tab che non compare più nella barra.
  $effect(() => {
    if (activeTab === "revisione" && reviewCount === 0) activeTab = "cronologia";
    if (activeTab === "cestino" && trashCount === 0) activeTab = "cronologia";
  });

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

  // "Esegui ora" vero e proprio è passato alla riga del remote in
  // RemoteRow.svelte (Simone, 22/8/2026): questo pannello si occupa solo di
  // provarne l'effetto prima di salvare, non di lanciarlo per davvero.
  async function tryDryRunBisync() {
    if (bisyncFormLocalPath.trim() === "") return;
    bisyncDryRunBusy = true;
    bisyncError = null;
    try {
      await persistBisync();
      await invoke("dry_run_bisync_job", { name: remoteName });
    } catch (error) {
      bisyncError = String(error);
    } finally {
      bisyncDryRunBusy = false;
      await onRefresh?.();
    }
  }

  async function forceBisyncNow() {
    bisyncBusy = true;
    bisyncError = null;
    try {
      await invoke("run_bisync_job_forced", { name: remoteName });
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
  <!-- Selettore di servizio sempre visibile, invece del vecchio link
       testuale "Cambia servizio (attuale: X)" nascosto dentro la scheda
       Configura — cambiare servizio resta raggiungibile anche mentre si
       guarda "Esegui e stato" o "Cronologia" (Simone, 21/8/2026: quel link
       era "poco evidente"). Stessa codifica cromatica per famiglia di
       servizio di .service-pill in RemoteRow.svelte. -->
  <div class="service-tabs">
    <button type="button" class="service-tab" class:mount={selectedService === "mount"} onclick={() => selectService("mount")}>
      <Icon kind="mount" />
      {$t("remoteRow.service.mount")}
    </button>
    <button type="button" class="service-tab" class:backup={selectedService === "backup"} onclick={() => selectService("backup")}>
      <Icon kind="backup" />
      {$t("remoteRow.service.backup")}
    </button>
    <button type="button" class="service-tab" class:bisync={selectedService === "bisync"} onclick={() => selectService("bisync")}>
      <Icon kind="bisync" />
      {$t("remoteRow.service.bisync")}
    </button>
  </div>

  <div class="tabs">
    <button type="button" class="tab" class:active={activeTab === "configura"} onclick={() => (activeTab = "configura")}>
      {$t("remotePanel.tabConfigure")}
    </button>
    {#if selectedService}
      <button type="button" class="tab" class:active={activeTab === "cronologia"} onclick={() => (activeTab = "cronologia")}>
        {$t("remotePanel.tabHistory")}
      </button>
      {#if selectedService === "bisync" && reviewCount > 0}
        <button type="button" class="tab" class:active={activeTab === "revisione"} onclick={() => (activeTab = "revisione")}>
          {$t("review.title")}
        </button>
      {/if}
      {#if selectedService !== "mount" && trashCount > 0}
        <button type="button" class="tab" class:active={activeTab === "cestino"} onclick={() => (activeTab = "cestino")}>
          {$t("trash.title")}
        </button>
      {/if}
    {/if}
  </div>

  <div class="tab-content">
    {#if pendingServiceSwitch}
      <!-- Dentro .tab-content (altezza fissa, vedi sotto) invece che tra
           i due gruppi di tab come prima: comparendo solo quando si passa
           da un servizio attivo a un altro tramite i pulsanti Mount/
           Backup/Sincronizzazione, da fuori faceva "saltare" il modal
           proprio in quel percorso e non negli altri (Simone, 22/8/2026). -->
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
    {:else if activeTab === "configura"}
      {#if selectedService === null}
        <p class="hint">{$t("remotePanel.pickServiceHint")}</p>
        <ul class="service-hints">
          <li><button type="button" class="service-hint-link mount" onclick={() => selectService("mount")}>{$t("remoteRow.service.mount")}</button> — {$t("remoteRow.mountAction")}</li>
          <li><button type="button" class="service-hint-link backup" onclick={() => selectService("backup")}>{$t("remoteRow.service.backup")}</button> — {$t("remoteRow.backupAction")}</li>
          <li><button type="button" class="service-hint-link bisync" onclick={() => selectService("bisync")}>{$t("remoteRow.service.bisync")}</button> — {$t("remoteRow.bisyncAction")}</li>
        </ul>
      {:else}
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
            {#if syncJob?.lastDryRun && syncJob.lastDryRun.whenUnix > (syncJob.history[0]?.whenUnix ?? 0)}
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
            {#if backupError}
              <LogView text={backupError} />
            {/if}
            <div class="row-actions modal-actions">
              <button type="button" onclick={tryDryRunBackup} disabled={backupDryRunBusy || backupBusy || backupFormLocalPath.trim() === ""}>
                {backupDryRunBusy ? $t("remoteRow.dryRunInProgress") : $t("remoteRow.dryRunButton")}
              </button>
              <button type="button" class="btn-primary" onclick={saveBackup} disabled={backupBusy || backupDryRunBusy || backupFormLocalPath.trim() === ""}>
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
            {#if bisyncJob?.lastDryRun && bisyncJob.lastDryRun.whenUnix > (bisyncJob.history[0]?.whenUnix ?? 0)}
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
            {#if bisyncError}
              <LogView text={bisyncError} />
            {/if}
            <div class="row-actions modal-actions">
              <button type="button" onclick={tryDryRunBisync} disabled={bisyncDryRunBusy || bisyncBusy || bisyncFormLocalPath.trim() === ""}>
                {bisyncDryRunBusy ? $t("remoteRow.dryRunInProgress") : $t("remoteRow.dryRunButton")}
              </button>
              <button type="button" class="btn-primary" onclick={saveBisync} disabled={bisyncBusy || bisyncDryRunBusy || bisyncFormLocalPath.trim() === ""}>
                {bisyncBusy ? $t("remoteRow.saving") : $t("remoteRow.saveChanges")}
              </button>
            </div>
          </div>
        {/if}
      {/if}
    {:else if activeTab === "cronologia" && selectedService}
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
                {#if entry.conflictPaths.length > 0 && index === 0}
                  <p class="hint">{$t("remoteRow.noVersionLost")}</p>
                {/if}
                {#if entry.duplicateNames.length > 0}
                  <p class="hint">
                    {$t("remoteRow.duplicateNamesFound", { values: { count: entry.duplicateNames.length } })}
                  </p>
                  <ul class="duplicate-names-list">
                    {#each entry.duplicateNames as dupName (dupName)}
                      <li>
                        <button type="button" class="duplicate-name-link" class:resolved={resolvedDuplicateNames.has(dupName)} onclick={() => examineDuplicate(dupName)}>
                          {#if resolvedDuplicateNames.has(dupName)}✓ {/if}{dupName}
                        </button>
                      </li>
                    {/each}
                  </ul>
                {/if}
                {#if entry.autoResynced}
                  <p class="hint">{$t("remoteRow.autoResyncedHint")}</p>
                {/if}
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
    {:else if activeTab === "revisione" && selectedService === "bisync" && bisyncJob}
      <ReviewView {remoteName} localRoot={bisyncLocalPathOf(bisyncJob)} path1={bisyncJob.path1} path2={bisyncJob.path2} {onRefresh} />
    {:else if activeTab === "cestino" && selectedService === "backup" && syncJob}
      <TrashView sides={[syncJob.destination]} />
    {:else if activeTab === "cestino" && selectedService === "bisync" && bisyncJob}
      <TrashView sides={[bisyncJob.path1, bisyncJob.path2]} />
    {/if}
  </div>
</div>

<RemoteFolderPicker bind:open={remotePickerOpen} {remoteName} onSelect={(path) => remotePickerOnSelect?.(path)} />

{#if bisyncJob}
  <DuplicateGroupModal
    bind:open={duplicateModalOpen}
    {remoteName}
    localRoot={bisyncLocalPathOf(bisyncJob)}
    path1={bisyncJob.path1}
    path2={bisyncJob.path2}
    name={examiningDuplicateName ?? ""}
    {onRefresh}
    onResolved={handleDuplicateResolved}
  />
{/if}

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

/* Altezza fissa invece di min-height: senza un tetto il box dell'intero
   modal (centrato in verticale da Modal.svelte) cresce o si restringe a
   ogni cambio di scheda, "saltando" sullo schermo — con un'altezza fissa e
   lo scroll qui dentro, a scorrere è solo il contenuto (tipicamente la
   Cronologia con molte voci), il modal resta fermo (Simone, 22/8/2026). */
/* Niente più altezza fissa con scroll proprio (era un ripiego per stare
   dentro la scatola del Modal, vedi il commento in cima al file): in una
   pagina normale il contenuto scorre con la pagina stessa
   (`.app-body` in +layout.svelte), come le altre pagine dell'app.
   `min-height` solo per non far collassare bruscamente il layout passando
   a una scheda molto corta (es. Cestino vuoto). */
.tab-content {
  min-height: 14em;
}

.service-tabs {
  display: flex;
  gap: 0.6em;
}

.service-tab {
  display: inline-flex;
  align-items: center;
  gap: 0.5em;
  padding: 0.6em 1em;
  border-radius: 10px;
  border: 1px solid var(--border-color-subtle);
  background-color: var(--surface-tint);
  color: var(--text-muted);
  font-weight: 600;
  font-size: 0.9em;
}

.service-tab.mount {
  color: var(--accent);
  background-color: var(--accent-bg);
  border-color: var(--accent);
}

.service-tab.backup {
  color: var(--blue);
  background-color: var(--blue-bg);
  border-color: var(--blue);
}

.service-tab.bisync {
  color: var(--violet);
  background-color: var(--violet-bg);
  border-color: var(--violet);
}

.service-hints {
  display: flex;
  flex-direction: column;
  gap: 0.4em;
  margin: 0.8em 0 0;
  padding: 0;
  list-style: none;
  color: var(--text-muted);
  font-size: 0.9em;
}

.service-hint-link {
  background: none;
  border: none;
  box-shadow: none;
  padding: 0;
  margin: 0;
  font: inherit;
  font-weight: 700;
  text-decoration: underline;
  cursor: pointer;
}

.service-hint-link.mount {
  color: var(--accent);
}

.service-hint-link.backup {
  color: var(--blue);
}

.service-hint-link.bisync {
  color: var(--violet);
}

.duplicate-names-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.2em;
}

.duplicate-name-link {
  background: none;
  border: none;
  box-shadow: none;
  padding: 0;
  margin: 0;
  font: inherit;
  font-size: 0.85em;
  color: var(--accent);
  text-decoration: underline;
  cursor: pointer;
  text-align: left;
  word-break: break-word;
}

.duplicate-name-link.resolved {
  color: var(--text-muted);
}

</style>
