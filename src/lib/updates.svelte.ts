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

// Solo in sviluppo: `VITE_FAKE_UPDATE=9.9.9 npm run tauri dev` simula un
// aggiornamento disponibile (icona nella barra del titolo, badge e voce
// nella tray, modal) senza toccare la rete. L'installazione vera fallisce
// con un messaggio esplicito. Nelle build di rilascio `import.meta.env.DEV`
// è falso e questo ramo sparisce.
function fakeUpdate(): Update | null {
  const version = import.meta.env.DEV ? import.meta.env.VITE_FAKE_UPDATE : undefined;
  if (!version) return null;
  return {
    version,
    downloadAndInstall: async () => {
      throw new Error("aggiornamento simulato (VITE_FAKE_UPDATE): nessun pacchetto da installare");
    },
  } as unknown as Update;
}

// Riusata sia dal controllo automatico all'avvio sia da quello manuale nelle
// impostazioni — un solo posto che parla col plugin updater, stato
// condiviso invece di duplicare la chiamata in più componenti.
export async function checkForUpdates(): Promise<void> {
  state = { status: "checking" };
  try {
    const fake = fakeUpdate();
    const isFake = fake !== null;
    const update = fake ?? (await check());
    if (!update) {
      state = { status: "up-to-date" };
      await reportToTray(null);
      return;
    }
    // Un aggiornamento simulato si comporta come su un AppImage (salvo
    // VITE_FAKE_INSTALL_KIND), altrimenti in sviluppo — dove `APPIMAGE` non
    // c'è — comparirebbe sempre "apri la pagina di download".
    const installKind = isFake
      ? ((import.meta.env.VITE_FAKE_INSTALL_KIND as InstallKind | undefined) ?? "appimage")
      : await invoke<InstallKind>("installation_kind");
    state = { status: "available", update, installKind };
    await reportToTray(update.version);
  } catch (error) {
    state = { status: "error", message: String(error) };
  }
}
