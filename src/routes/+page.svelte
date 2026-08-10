<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
  import { goto } from "$app/navigation";
  import RemoteRow from "$lib/components/RemoteRow.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import ImportRemotesPanel from "$lib/components/ImportRemotesPanel.svelte";
  import PasswordField from "$lib/components/PasswordField.svelte";
  import type { MountEntry, SyncJob, BisyncJob } from "$lib/types";

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
      startupWarning = `rclone non è disponibile: ${error}`;
    });
    invoke<string>("rcd_status").catch((error) => {
      startupWarning = `Il servizio interno di rclone non risponde: ${error}`;
    });
    loadOwnRemotes();
    loadServices();
    // Solo per l'etichetta del bottone ("Imposta password" vs "Gestisci
    // password", richiesta di Simone) — lo stato effettivo per il
    // contenuto del modal viene comunque riletto fresco ad ogni apertura
    // in openPasswordModal().
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
      passwordError = "Inserisci la password attuale.";
      return;
    }
    if (passwordNew.length < 8) {
      passwordError = "La password deve avere almeno 8 caratteri.";
      return;
    }
    if (passwordNew !== passwordConfirm) {
      passwordError = "Le due password non coincidono.";
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
      passwordError = "Inserisci la password attuale per rimuoverla.";
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
  // incorporato, sostituisce la navigazione verso /importa-remote di
  // prima — restare dentro il modal invece di "aprire una finestra" a
  // parte, richiesta esplicita di Simone.
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
      if (!confirm("Senza password il backup sarà leggibile in chiaro da chiunque apra il file. Continuare comunque?")) {
        return;
      }
      exportPasswordModalOpen = false;
      await exportBackup(undefined);
      return;
    }
    if (exportPassword.length < 8) {
      exportPasswordError = "La password deve avere almeno 8 caratteri, oppure lasciala vuota per non cifrare il backup.";
      return;
    }
    if (exportPassword !== exportPasswordConfirm) {
      exportPasswordError = "Le due password non coincidono.";
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
      title: "Salva backup di Rclone Easy",
      defaultPath: password ? "rclone-easy-backup.age" : "rclone-easy-backup.json",
      filters: password
        ? [{ name: "Backup Rclone Easy (cifrato)", extensions: ["age"] }]
        : [{ name: "Backup Rclone Easy (in chiaro)", extensions: ["json"] }],
    });
    if (!destination) return;
    importExportBusy = true;
    try {
      await invoke("export_backup", { destinationPath: destination, password: password ?? null });
      importExportOk = password ? `Backup cifrato salvato in ${destination}` : `Backup in chiaro salvato in ${destination}`;
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
      title: "Scegli un backup di Rclone Easy da ripristinare",
      multiple: false,
      filters: [{ name: "Backup Rclone Easy", extensions: ["age", "json"] }],
    });
    if (typeof source !== "string") return;
    if (
      !confirm(
        "Ripristinare questo backup sovrascrive tutti i remote attuali di Rclone Easy (non mount/backup/sincronizzazioni, che restano quelli già configurati su questo PC). L'operazione non è reversibile. Continuare?",
      )
    ) {
      return;
    }
    pendingImportSource = source;
    importPassword = "";
    importPasswordModalOpen = true;
  }

  let restartNeededModalOpen = $state(false);

  async function confirmImportPassword() {
    const source = pendingImportSource;
    if (!source) return;
    importPasswordModalOpen = false;
    importExportBusy = true;
    try {
      await invoke("import_backup", { sourcePath: source, password: importPassword.trim() === "" ? null : importPassword });
      importExportModalOpen = false;
      restartNeededModalOpen = true;
      await Promise.all([loadOwnRemotes(), loadServices()]);
    } catch (error) {
      importExportError = String(error);
    } finally {
      importExportBusy = false;
      pendingImportSource = null;
    }
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
      <button type="button" onclick={() => goto("/nuovo-remote")}>Aggiungi Remote</button>
      <button type="button" onclick={openPasswordModal}>{passwordAlreadySet ? "Gestisci password" : "Imposta password"}</button>
      <button type="button" onclick={openImportExportModal}>Importa / Esporta…</button>
    </div>

    {#if ownRemotes.status === "loading"}
      <p>Verifica in corso…</p>
    {:else if ownRemotes.status === "error"}
      <p class="error">✗ {ownRemotes.message}</p>
    {:else if ownRemotes.value.length === 0}
      <p class="empty">Nessun remote ancora creato.</p>
    {:else}
      <ul class="remote-list">
        {#each ownRemotes.value as name (name)}
          <RemoteRow remoteName={name} {mounts} {jobs} {bisyncJobs} onDeleted={loadOwnRemotes} onRefresh={loadServices} />
        {/each}
      </ul>
    {/if}
  </section>
</main>

<Modal bind:open={passwordModalOpen} title="Password della configurazione">
  <div class="stack-form">
    {#if passwordAlreadySet}
      <p class="hint">
        La configurazione è già protetta da una password. Inserisci quella attuale per cambiarla o rimuoverla.
      </p>
    {:else}
      <p class="hint">
        Protegge <code>rclone.conf</code> (dove sono salvate le credenziali dei tuoi remote) con una password: senza
        non si può avviare l'app. Facoltativo, ma consigliato. Se la dimentichi non c'è modo di recuperarla.
      </p>
    {/if}
    {#if passwordAlreadySet}
      <PasswordField bind:value={passwordCurrent} label="Password attuale" disabled={passwordBusy} />
    {/if}
    <PasswordField
      bind:value={passwordNew}
      label={passwordAlreadySet ? "Nuova password" : "Password"}
      placeholder="Almeno 8 caratteri"
      disabled={passwordBusy}
    />
    <PasswordField bind:value={passwordConfirm} label="Conferma password" disabled={passwordBusy} />
    {#if passwordError}
      <p class="error">✗ {passwordError}</p>
    {/if}
    <div class="stack-actions">
      {#if passwordAlreadySet}
        <button type="button" class="link-button" onclick={removePassword} disabled={passwordBusy}>Rimuovi password</button>
      {/if}
      <button type="button" onclick={submitPassword} disabled={passwordBusy}>
        {passwordBusy ? "In corso…" : passwordAlreadySet ? "Cambia password" : "Imposta password"}
      </button>
    </div>
  </div>
</Modal>

<Modal bind:open={importExportModalOpen} title="Importa / Esporta">
  {#if importExportView === "menu"}
    <div class="import-export-menu">
      <button type="button" class="menu-option" onclick={() => (importExportView = "import-remotes")}>
        <strong>Importa remote esistenti</strong>
        <span>Porta nella config di Rclone Easy i remote già configurati con rclone.</span>
      </button>
      <button type="button" class="menu-option" onclick={openExportPasswordModal} disabled={importExportBusy}>
        <strong>Crea backup Rclone Easy</strong>
        <span>Salva i remote configurati (non mount/backup/sincronizzazioni, legati a cartelle di questo PC), opzionalmente cifrati con una password a tua scelta.</span>
      </button>
      <button type="button" class="menu-option" onclick={restoreBackup} disabled={importExportBusy}>
        <strong>Ripristina backup Rclone Easy</strong>
        <span>Sovrascrive i remote attuali con quelli di un file di backup.</span>
      </button>
      {#if importExportBusy}
        <p class="hint">In corso…</p>
      {/if}
      {#if importExportOk}
        <p class="ok">✓ {importExportOk}</p>
      {/if}
      {#if importExportError}
        <p class="error">✗ {importExportError}</p>
      {/if}
    </div>
  {:else}
    <button type="button" class="link-button back-button" onclick={() => (importExportView = "menu")}>← Indietro</button>
    <ImportRemotesPanel onImported={loadOwnRemotes} />
  {/if}
</Modal>

<Modal bind:open={exportPasswordModalOpen} title="Password del backup">
  <div class="stack-form">
    <p class="hint">
      Il backup contiene le credenziali dei tuoi remote: scegli una password per cifrarlo. Ti servirà la stessa
      password per ripristinarlo in futuro — se la perdi, il backup diventa inutilizzabile. Puoi anche lasciare
      entrambi i campi vuoti: il backup verrà salvato in chiaro, leggibile da chiunque apra il file.
    </p>
    <label class="stack-field">
      Password (facoltativa)
      <input type="password" bind:value={exportPassword} placeholder="Almeno 8 caratteri, o vuoto per non cifrare" />
    </label>
    <label class="stack-field">
      Conferma password
      <input type="password" bind:value={exportPasswordConfirm} />
    </label>
    {#if exportPasswordError}
      <p class="error">✗ {exportPasswordError}</p>
    {/if}
    <div class="stack-actions">
      <button type="button" onclick={confirmExportPassword}>Continua</button>
    </div>
  </div>
</Modal>

<Modal bind:open={importPasswordModalOpen} title="Password del backup">
  <div class="stack-form">
    <p class="hint">
      Inserisci la password scelta al momento del backup. Lascia vuoto se il backup è stato salvato in chiaro,
      senza password.
    </p>
    <label class="stack-field">
      Password
      <input type="password" bind:value={importPassword} />
    </label>
    <div class="stack-actions">
      <button type="button" onclick={confirmImportPassword}>Ripristina</button>
    </div>
  </div>
</Modal>

<Modal bind:open={restartNeededModalOpen} title="L'applicazione dev'essere riavviata">
  <div class="stack-form">
    <p class="hint">Il backup è stato ripristinato. Riavvia l'app perché i cambiamenti abbiano pieno effetto.</p>
    <div class="stack-actions">
      <button type="button" onclick={restartNow}>Riavvia ora</button>
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
