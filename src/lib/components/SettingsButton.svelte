<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
  import { isEnabled as isAutostartEnabled, enable as enableAutostart, disable as disableAutostart } from "@tauri-apps/plugin-autostart";
  import Modal from "./Modal.svelte";
  import Icon from "./Icon.svelte";
  import ImportRemotesPanel from "./ImportRemotesPanel.svelte";
  import PasswordField from "./PasswordField.svelte";
  import { t } from "$lib/i18n";
  import { updateState, checkForUpdates } from "$lib/updates.svelte";

  let { onRemotesChanged }: { onRemotesChanged: () => void } = $props();

  let open = $state(false);

  // Cliccando "Impostazioni" nel menu della tray, il backend porta la
  // finestra in primo piano ed emette questo evento — vedi
  // tray.rs::open_settings. Assume che il componente sia montato, cioè che
  // l'utente si trovi sulla home: stessa semplificazione già accettata per
  // FOCUS_REMOTE_EVENT in RemoteRow.svelte.
  $effect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen("rclone-easy://open-settings", () => (open = true)).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  });

  // --- Avvio automatico e ridotto a icona ---
  let autostartLoaded = $state(false);
  let autostart = $state(false);
  let startMinimized = $state(false);

  $effect(() => {
    if (open && !autostartLoaded) {
      autostartLoaded = true;
      isAutostartEnabled()
        .then((v) => (autostart = v))
        .catch(() => (autostart = false));
      invoke<{ startMinimized: boolean }>("get_app_settings")
        .then((s) => (startMinimized = s.startMinimized))
        .catch(() => (startMinimized = false));
    }
  });

  async function onAutostartChange() {
    try {
      if (autostart) await enableAutostart();
      else await disableAutostart();
    } catch {
      // Ripristina lo stato reale del sistema operativo se l'operazione
      // fallisce (es. permessi mancanti), invece di lasciare il checkbox
      // disallineato da quanto è davvero attivo.
      autostart = await isAutostartEnabled().catch(() => false);
    }
  }

  async function onStartMinimizedChange() {
    try {
      await invoke("set_start_minimized", { enabled: startMinimized });
    } catch {
      startMinimized = !startMinimized;
    }
  }

  // --- Password della configurazione (rclone.conf), diversa da quella del
  // singolo file di backup esportato: protegge la config live sul disco,
  // vedi config_password.rs. ---
  let passwordAlreadySet = $state(false);

  $effect(() => {
    // Solo per l'etichetta della voce di menu ("Imposta password" vs
    // "Gestisci password") — lo stato effettivo per il contenuto del modal
    // viene comunque riletto fresco ad ogni apertura in openPasswordModal().
    invoke<boolean>("config_password_status")
      .then((value) => (passwordAlreadySet = value))
      .catch(() => {});
  });

  let passwordModalOpen = $state(false);
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
      onRemotesChanged();
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

<button type="button" class="settings-trigger" onclick={() => (open = true)}>
  <Icon kind="settings" />
  {$t("settings.title")}
</button>

<Modal bind:open title={$t("settings.title")}>
  <div class="settings-menu">
    <button type="button" class="menu-option" onclick={openPasswordModal}>
      <strong>{passwordAlreadySet ? $t("home.managePassword") : $t("home.setPassword")}</strong>
      <span>{$t("settings.passwordDesc")}</span>
    </button>
    <button type="button" class="menu-option" onclick={openImportExportModal}>
      <strong>{$t("home.importExport")}</strong>
      <span>{$t("settings.importExportDesc")}</span>
    </button>
  </div>
  <div class="settings-toggles">
    <label class="checkbox-row">
      <input type="checkbox" bind:checked={autostart} onchange={onAutostartChange} />
      {$t("settings.autostart")}
    </label>
    <p class="hint">{$t("settings.autostartHint")}</p>
    <label class="checkbox-row">
      <input type="checkbox" bind:checked={startMinimized} onchange={onStartMinimizedChange} />
      {$t("settings.startMinimized")}
    </label>
    <p class="hint">{$t("settings.startMinimizedHint")}</p>
  </div>
  <div class="settings-toggles">
    <div class="update-check-row">
      <span>
        {#if updateState().status === "checking"}
          {$t("update.checking")}
        {:else if updateState().status === "up-to-date"}
          {$t("update.upToDate")}
        {:else if updateState().status === "available"}
          {@const s = updateState()}
          {#if s.status === "available"}
            {$t("update.newVersion", { values: { version: s.update.version } })}
          {/if}
        {:else if updateState().status === "skipped"}
          {@const s = updateState()}
          {#if s.status === "skipped"}
            {$t("update.versionSkipped", { values: { version: s.update.version } })}
          {/if}
        {:else if updateState().status === "error"}
          {$t("update.checkFailed")}
        {:else}
          {$t("update.neverChecked")}
        {/if}
      </span>
      <button
        type="button"
        class="link-button"
        onclick={() => checkForUpdates({ manual: true })}
        disabled={updateState().status === "checking"}
      >
        {$t("update.checkNow")}
      </button>
    </div>
  </div>
</Modal>

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
    <ImportRemotesPanel onImported={onRemotesChanged} />
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
.settings-trigger {
  display: inline-flex;
  align-items: center;
  gap: 0.4em;
}

.settings-menu {
  display: flex;
  flex-direction: column;
  gap: 0.6em;
  margin-bottom: 1em;
}

.settings-toggles {
  display: flex;
  flex-direction: column;
  gap: 0.3em;
  padding-top: 0.8em;
  border-top: 1px solid var(--border-color-subtle);
}

.update-check-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.6em;
  font-size: 0.9em;
}

.update-check-row span {
  color: var(--text-muted);
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
