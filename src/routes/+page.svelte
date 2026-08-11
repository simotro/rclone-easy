<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
  import { goto } from "$app/navigation";
  import RemoteRow from "$lib/components/RemoteRow.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import ImportRemotesPanel from "$lib/components/ImportRemotesPanel.svelte";
  import PasswordField from "$lib/components/PasswordField.svelte";
  import type { MountEntry, SyncJob, BisyncJob } from "$lib/types";
  import { t } from "$lib/i18n";

  type CheckState<T> = { status: "loading" } | { status: "ok"; value: T } | { status: "error"; message: string };

  let ownRemotes = $state<CheckState<string[]>>({ status: "loading" });
  // Nessuna sezione diagnostica dedicata: qui basta sapere se qualcosa di
  // fondamentale non funziona (rclone non trovato, demone rcd non
  // partito) — senza uno dei due l'app è sostanzialmente inutilizzabile,
  // quindi un avviso silenzioso-se-tutto-ok è sufficiente, niente da
  // esplorare in un pannello a parte.
  let startupWarning = $state<string | null>(null);

  async function runCheck<T>(command: string): Promise<CheckState<T>> {
    try {
      const value = await invoke<T>(command);
      return { status: "ok", value };
    } catch (error) {
      return { status: "error", message: String(error) };
    }
  }

  async function loadOwnRemotes() {
    ownRemotes = await runCheck<string[]>("list_own_remotes");
  }

  // Mount/backup/bisync caricati UNA volta qui, condivisi da tutte le righe
  // (RemoteRow non fa più fetch proprio): con N remote, invoke tre liste
  // complete una volta sola invece di farlo N volte con lo stesso risultato
  // — indispensabile per poter aggiornare periodicamente (serve per il
  // countdown "prossima esecuzione") senza far scalare il costo con il
  // numero di remote.
  let mounts = $state<MountEntry[]>([]);
  let jobs = $state<SyncJob[]>([]);
  let bisyncJobs = $state<BisyncJob[]>([]);

  async function loadServices() {
    try {
      const [m, j, b] = await Promise.all([
        invoke<MountEntry[]>("list_mounts"),
        invoke<SyncJob[]>("list_jobs"),
        invoke<BisyncJob[]>("list_bisync_jobs"),
      ]);
      mounts = m;
      jobs = j;
      bisyncJobs = b;
    } catch {
      // Silenzioso: ogni RemoteRow mostra comunque lo stato "nessuna
      // operazione" se le liste restano vuote, non serve un banner
      // separato per un fallimento di un aggiornamento periodico.
    }
  }

  $effect(() => {
    invoke<string>("check_rclone_installed").catch((error) => {
      startupWarning = $t("home.rcloneNotAvailable", { values: { error } });
    });
    invoke<string>("rcd_status").catch((error) => {
      startupWarning = $t("home.rcdNotResponding", { values: { error } });
    });
    loadOwnRemotes();
    loadServices();
    // Solo per l'etichetta del bottone ("Imposta password" vs "Gestisci
    // password") — lo stato effettivo per il contenuto del modal viene
    // comunque riletto fresco ad ogni apertura in openPasswordModal().
    invoke<boolean>("config_password_status")
      .then((value) => (passwordAlreadySet = value))
      .catch(() => {});
    // 10s: abbastanza spesso da tenere il countdown della prossima
    // esecuzione automatica ragionevolmente aggiornato, abbastanza raro da
    // non pesare (tre chiamate IPC leggere ogni 10s, indipendentemente da
    // quanti remote ci sono).
    const id = setInterval(loadServices, 10000);
    return () => clearInterval(id);
  });

  // --- Password della configurazione (rclone.conf), diversa da quella del
  // singolo file di backup esportato: protegge la config live sul disco,
  // vedi config_password.rs. ---
  let passwordModalOpen = $state(false);
  let passwordAlreadySet = $state(false);
  let passwordCurrent = $state("");
  let passwordNew = $state("");
  let passwordConfirm = $state("");
  let passwordBusy = $state(false);
  let passwordError = $state<string | null>(null);

  async function openPasswordModal() {
    passwordCurrent = "";
    passwordNew = "";
    passwordConfirm = "";
    passwordError = null;
    passwordModalOpen = true;
    try {
      passwordAlreadySet = await invoke<boolean>("config_password_status");
    } catch {
      passwordAlreadySet = false;
    }
  }

  async function submitPassword() {
    if (passwordAlreadySet && passwordCurrent.trim() === "") {
      passwordError = $t("home.enterCurrentPassword");
      return;
    }
    if (passwordNew.length < 8) {
      passwordError = $t("home.passwordMinLength");
      return;
    }
    if (passwordNew !== passwordConfirm) {
      passwordError = $t("home.passwordsDontMatch");
      return;
    }
    passwordBusy = true;
    passwordError = null;
    try {
      await invoke("set_config_password", {
        currentPassword: passwordAlreadySet ? passwordCurrent : null,
        newPassword: passwordNew,
      });
      passwordModalOpen = false;
    } catch (error) {
      passwordError = String(error);
    } finally {
      passwordBusy = false;
    }
  }

  async function removePassword() {
    if (passwordCurrent.trim() === "") {
      passwordError = $t("home.enterCurrentPasswordToRemove");
      return;
    }
    passwordBusy = true;
    passwordError = null;
    try {
      await invoke("remove_config_password", { currentPassword: passwordCurrent });
      passwordModalOpen = false;
    } catch (error) {
      passwordError = String(error);
    } finally {
      passwordBusy = false;
    }
  }

  let importExportModalOpen = $state(false);
  // "menu" = le 3 opzioni; "import-remotes" = il pannello di import
  // incorporato, resta dentro il modal invece di navigare verso una
  // pagina a parte.
  let importExportView = $state<"menu" | "import-remotes">("menu");
  let importExportBusy = $state(false);
  let importExportError = $state<string | null>(null);
  let importExportOk = $state<string | null>(null);

  function openImportExportModal() {
    importExportView = "menu";
    importExportError = null;
    importExportOk = null;
    importExportModalOpen = true;
  }

  // --- Password del backup (export/import) ---
  // Il bundle contiene solo rclone.conf (i remote, non mount/backup/bisync:
  // quelli dipendono da cartelle locali specifiche di questa macchina). Le
  // credenziali che rclone "offusca" nel file sono reversibili da chiunque
  // conosca l'algoritmo, non una cifratura vera — una password è quindi
  // consigliata, ma facoltativa: lasciarla vuota salva il backup in chiaro,
  // scelta esplicita dell'utente dopo essere stato avvisato.
  let exportPasswordModalOpen = $state(false);
  let exportPassword = $state("");
  let exportPasswordConfirm = $state("");
  let exportPasswordError = $state<string | null>(null);

  function openExportPasswordModal() {
    exportPassword = "";
    exportPasswordConfirm = "";
    exportPasswordError = null;
    exportPasswordModalOpen = true;
  }

  async function confirmExportPassword() {
    if (exportPassword === "" && exportPasswordConfirm === "") {
      if (!confirm($t("home.exportNoPasswordConfirm"))) {
        return;
      }
      exportPasswordModalOpen = false;
      await exportBackup(undefined);
      return;
    }
    if (exportPassword.length < 8) {
      exportPasswordError = $t("home.exportPasswordMinLength");
      return;
    }
    if (exportPassword !== exportPasswordConfirm) {
      exportPasswordError = $t("home.passwordsDontMatch");
      return;
    }
    const password = exportPassword;
    exportPasswordModalOpen = false;
    await exportBackup(password);
  }

  async function exportBackup(password: string | undefined) {
    importExportError = null;
    importExportOk = null;
    const destination = await saveFileDialog({
      title: $t("home.saveBackupDialogTitle"),
      defaultPath: password ? "rclone-easy-backup.age" : "rclone-easy-backup.json",
      filters: password
        ? [{ name: $t("home.encryptedBackupFilterName"), extensions: ["age"] }]
        : [{ name: $t("home.plainBackupFilterName"), extensions: ["json"] }],
    });
    if (!destination) return;
    importExportBusy = true;
    try {
      await invoke("export_backup", { destinationPath: destination, password: password ?? null });
      importExportOk = password
        ? $t("home.encryptedBackupSaved", { values: { destination } })
        : $t("home.plainBackupSaved", { values: { destination } });
    } catch (error) {
      importExportError = String(error);
    } finally {
      importExportBusy = false;
    }
  }

  let importPasswordModalOpen = $state(false);
  let importPassword = $state("");
  let pendingImportSource = $state<string | null>(null);

  async function restoreBackup() {
    importExportError = null;
    importExportOk = null;
    const source = await openFileDialog({
      title: $t("home.chooseBackupDialogTitle"),
      multiple: false,
      filters: [{ name: $t("home.backupFilterName"), extensions: ["age", "json"] }],
    });
    if (typeof source !== "string") return;
    if (!confirm($t("home.restoreConfirm"))) {
      return;
    }
    // Si tenta prima senza password: un backup salvato in chiaro (nessuna
    // password scelta in fase di export) va importato subito, senza
    // interrompere l'utente con un modal inutile. Solo se il backend segnala
    // esplicitamente che il file è cifrato si chiede la password.
    pendingImportSource = source;
    await attemptImport(source, null);
  }

  let restartNeededModalOpen = $state(false);

  async function attemptImport(source: string, password: string | null) {
    importExportBusy = true;
    try {
      await invoke("import_backup", { sourcePath: source, password });
      importExportModalOpen = false;
      restartNeededModalOpen = true;
      pendingImportSource = null;
      await Promise.all([loadOwnRemotes(), loadServices()]);
    } catch (error) {
      const message = String(error);
      if (password === null && message.includes("è cifrato")) {
        importPassword = "";
        importPasswordModalOpen = true;
      } else {
        importExportError = message;
        pendingImportSource = null;
      }
    } finally {
      importExportBusy = false;
    }
  }

  async function confirmImportPassword() {
    const source = pendingImportSource;
    if (!source) return;
    importPasswordModalOpen = false;
    await attemptImport(source, importPassword.trim() === "" ? null : importPassword);
  }

  async function restartNow() {
    await invoke("restart_app");
  }
</script>

<main class="container">
  <div class="app-title">
    <img src="/app-icon.png" alt="" class="app-icon" />
    <h1>Rclone Easy</h1>
  </div>

  {#if startupWarning}
    <p class="startup-warning">⚠ {startupWarning}</p>
  {/if}

  <section>
    <div class="top-actions">
      <button type="button" onclick={() => goto("/nuovo-remote")}>{$t("home.addRemote")}</button>
      <button type="button" onclick={openPasswordModal}>{passwordAlreadySet ? $t("home.managePassword") : $t("home.setPassword")}</button>
      <button type="button" onclick={openImportExportModal}>{$t("home.importExport")}</button>
    </div>

    {#if ownRemotes.status === "loading"}
      <p>{$t("home.checking")}</p>
    {:else if ownRemotes.status === "error"}
      <p class="error">✗ {ownRemotes.message}</p>
    {:else if ownRemotes.value.length === 0}
      <p class="empty">{$t("home.noRemotesYet")}</p>
    {:else}
      <ul class="remote-list">
        {#each ownRemotes.value as name (name)}
          <RemoteRow remoteName={name} {mounts} {jobs} {bisyncJobs} onDeleted={loadOwnRemotes} onRefresh={loadServices} />
        {/each}
      </ul>
    {/if}
  </section>
</main>

<Modal bind:open={passwordModalOpen} title={$t("home.passwordModalTitle")}>
  <div class="stack-form">
    {#if passwordAlreadySet}
      <p class="hint">
        {$t("home.passwordAlreadySetHint")}
      </p>
    {:else}
      <p class="hint">
        {$t("home.passwordHintBefore")} <code>rclone.conf</code>{$t("home.passwordHintAfter")}
      </p>
    {/if}
    {#if passwordAlreadySet}
      <PasswordField bind:value={passwordCurrent} label={$t("home.currentPasswordLabel")} disabled={passwordBusy} />
    {/if}
    <PasswordField
      bind:value={passwordNew}
      label={passwordAlreadySet ? $t("home.newPasswordLabel") : $t("unlock.passwordLabel")}
      placeholder={$t("home.minEightChars")}
      disabled={passwordBusy}
    />
    <PasswordField bind:value={passwordConfirm} label={$t("home.confirmPasswordLabel")} disabled={passwordBusy} />
    {#if passwordError}
      <p class="error">✗ {passwordError}</p>
    {/if}
    <div class="stack-actions">
      {#if passwordAlreadySet}
        <button type="button" class="link-button" onclick={removePassword} disabled={passwordBusy}>{$t("home.removePassword")}</button>
      {/if}
      <button type="button" onclick={submitPassword} disabled={passwordBusy}>
        {passwordBusy ? $t("common.inProgress") : passwordAlreadySet ? $t("home.changePassword") : $t("home.setPassword")}
      </button>
    </div>
  </div>
</Modal>

<Modal bind:open={importExportModalOpen} title={$t("home.importExportTitle")}>
  {#if importExportView === "menu"}
    <div class="import-export-menu">
      <button type="button" class="menu-option" onclick={() => (importExportView = "import-remotes")}>
        <strong>{$t("home.importExistingRemotes")}</strong>
        <span>{$t("home.importExistingRemotesDesc")}</span>
      </button>
      <button type="button" class="menu-option" onclick={openExportPasswordModal} disabled={importExportBusy}>
        <strong>{$t("home.createBackup")}</strong>
        <span>{$t("home.createBackupDesc")}</span>
      </button>
      <button type="button" class="menu-option" onclick={restoreBackup} disabled={importExportBusy}>
        <strong>{$t("home.restoreBackupOption")}</strong>
        <span>{$t("home.restoreBackupDesc")}</span>
      </button>
      {#if importExportBusy}
        <p class="hint">{$t("common.inProgress")}</p>
      {/if}
      {#if importExportOk}
        <p class="ok">✓ {importExportOk}</p>
      {/if}
      {#if importExportError}
        <p class="error">✗ {importExportError}</p>
      {/if}
    </div>
  {:else}
    <button type="button" class="link-button back-button" onclick={() => (importExportView = "menu")}>← {$t("home.back")}</button>
    <ImportRemotesPanel onImported={loadOwnRemotes} />
  {/if}
</Modal>

<Modal bind:open={exportPasswordModalOpen} title={$t("home.backupPasswordTitle")}>
  <div class="stack-form">
    <p class="hint">
      {$t("home.exportPasswordHint")}
    </p>
    <label class="stack-field">
      {$t("home.exportPasswordLabel")}
      <input type="password" bind:value={exportPassword} placeholder={$t("home.minEightCharsOrEmptyToSkipEncryption")} />
    </label>
    <label class="stack-field">
      {$t("home.confirmPasswordLabel")}
      <input type="password" bind:value={exportPasswordConfirm} />
    </label>
    {#if exportPasswordError}
      <p class="error">✗ {exportPasswordError}</p>
    {/if}
    <div class="stack-actions">
      <button type="button" onclick={confirmExportPassword}>{$t("home.continueButton")}</button>
    </div>
  </div>
</Modal>

<Modal bind:open={importPasswordModalOpen} title={$t("home.backupPasswordTitle")}>
  <div class="stack-form">
    <p class="hint">{$t("home.importPasswordHint")}</p>
    <label class="stack-field">
      {$t("unlock.passwordLabel")}
      <input type="password" bind:value={importPassword} />
    </label>
    <div class="stack-actions">
      <button type="button" onclick={confirmImportPassword}>{$t("home.restoreButton")}</button>
    </div>
  </div>
</Modal>

<Modal bind:open={restartNeededModalOpen} title={$t("home.restartTitle")}>
  <div class="stack-form">
    <p class="hint">{$t("home.restartHint")}</p>
    <div class="stack-actions">
      <button type="button" onclick={restartNow}>{$t("home.restartNow")}</button>
    </div>
  </div>
</Modal>

<style>
.container {
  margin: 0 auto;
  max-width: 44em;
  padding: 4vh 1.5em;
}

.app-title {
  display: flex;
  align-items: center;
  gap: 0.5em;
}

.app-icon {
  width: 4.4em;
  height: 4.4em;
}

.app-title h1 {
  margin: 0;
}

section {
  margin-top: 1.5em;
}

.empty {
  font-size: 0.9em;
}

.top-actions {
  display: flex;
  gap: 0.6em;
  flex-wrap: wrap;
  margin-bottom: 1em;
}

.remote-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 0.6em;
}

.startup-warning {
  margin-top: 1em;
  padding: 0.7em 1em;
  border-radius: 8px;
  background-color: var(--warning-bg);
  color: var(--warning-text);
  font-size: 0.9em;
}

.import-export-menu {
  display: flex;
  flex-direction: column;
  gap: 0.6em;
}

.menu-option {
  display: flex;
  flex-direction: column;
  gap: 0.2em;
  text-align: left;
  padding: 0.8em 1em;
  font-family: inherit;
}

.menu-option span {
  color: var(--text-muted);
  font-size: 0.85em;
  font-weight: 400;
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

.back-button {
  margin-bottom: 0.8em;
}

.stack-form {
  display: flex;
  flex-direction: column;
  gap: 0.7em;
}

.stack-field {
  display: flex;
  flex-direction: column;
  gap: 0.3em;
  font-weight: 500;
}

.stack-actions {
  display: flex;
  justify-content: flex-end;
  gap: 0.6em;
}

.stack-actions:has(.link-button) {
  justify-content: space-between;
  align-items: center;
}
</style>
