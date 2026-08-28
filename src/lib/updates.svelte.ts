import { check, type Update } from "@tauri-apps/plugin-updater";
import { invoke } from "@tauri-apps/api/core";

// "appimage"/"windows": l'updater di Tauri sa sostituire da sé il pacchetto
// in esecuzione. "package-manager" (.deb/.rpm, o piattaforme non
// riconosciute): l'installazione automatica non è possibile, l'unica azione
// sensata è aprire la pagina di download — vedi installation.rs.
export type InstallKind = "appimage" | "windows" | "package-manager";

export type UpdateCheckState =
  | { status: "idle" }
  | { status: "checking" }
  | { status: "up-to-date" }
  | { status: "available"; update: Update; installKind: InstallKind }
  | { status: "skipped"; update: Update; installKind: InstallKind }
  | { status: "error"; message: string };

let state = $state<UpdateCheckState>({ status: "idle" });

// Stato del modal di download/installazione (UpdateButton.svelte), sollevato
// qui invece che come variabile locale del componente: così anche
// SettingsButton.svelte può aprirlo direttamente ("Aggiorna adesso" al posto
// di "Controlla ora" quando un aggiornamento è già stato trovato) senza
// dover passare per l'evento Tauri "rclone-easy://open-update" (pensato per
// la tray, un processo separato che non ha altro modo di raggiungere la UI).
let modalOpen = $state(false);

export function isUpdateModalOpen(): boolean {
  return modalOpen;
}

export function setUpdateModalOpen(value: boolean): void {
  modalOpen = value;
}

// Non reattivo apposta (solo per un confronto interno, non per la UI): la
// tray non ha altro modo di sapere se c'è un aggiornamento se non tramite
// report_update_available (vedi update_state.rs) — richiamato ogni volta
// che questo modulo scopre o nasconde un aggiornamento, mai al contrario.
let skippedVersion: string | null = null;

export function updateState(): UpdateCheckState {
  return state;
}

async function reportToTray(version: string | null): Promise<void> {
  try {
    await invoke("report_update_available", { version });
  } catch {
    // La tray è solo un riflesso in più di questo stato: se il comando
    // fallisse (non dovrebbe) l'aggiornamento resta comunque gestibile da
    // qui, non c'è nulla di più sensato da fare che ignorare l'errore.
  }
}

// Riusata sia dal controllo automatico all'avvio sia da quello manuale nelle
// impostazioni — un solo posto che parla col plugin updater, stato
// condiviso invece di duplicare la chiamata in più componenti. `manual`
// (dalle Impostazioni, mai dal controllo automatico) fa ricomparire anche
// una versione già saltata in precedenza in questa sessione — richiesta
// esplicita di Simone (19/8/2026): "salta questa versione" nasconde
// l'avviso finché non si lancia di nuovo un controllo a mano.
export async function checkForUpdates(options: { manual?: boolean } = {}): Promise<void> {
  if (options.manual) skippedVersion = null;
  state = { status: "checking" };
  try {
    const update = await check();
    if (!update) {
      state = { status: "up-to-date" };
      await reportToTray(null);
      return;
    }
    const installKind = await invoke<InstallKind>("installation_kind");
    if (update.version === skippedVersion) {
      state = { status: "skipped", update, installKind };
      return;
    }
    state = { status: "available", update, installKind };
    await reportToTray(update.version);
  } catch (error) {
    state = { status: "error", message: String(error) };
  }
}

export async function skipUpdate(): Promise<void> {
  if (state.status !== "available") return;
  skippedVersion = state.update.version;
  state = { status: "skipped", update: state.update, installKind: state.installKind };
  await reportToTray(null);
}
