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

export function updateState(): UpdateCheckState {
  return state;
}

// Riusata sia dal controllo automatico all'avvio sia da quello manuale nelle
// impostazioni — un solo posto che parla col plugin updater, stato
// condiviso invece di duplicare la chiamata in più componenti.
export async function checkForUpdates(): Promise<void> {
  state = { status: "checking" };
  try {
    const update = await check();
    if (!update) {
      state = { status: "up-to-date" };
      return;
    }
    const installKind = await invoke<InstallKind>("installation_kind");
    state = { status: "available", update, installKind };
  } catch (error) {
    state = { status: "error", message: String(error) };
  }
}
