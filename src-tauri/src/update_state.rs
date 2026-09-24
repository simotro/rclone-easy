use std::sync::Mutex;

/// Ultima versione di aggiornamento nota, riportata dal frontend dopo un
/// controllo — il controllo vero e la vera installazione restano lato JS
/// (plugin updater di Tauri, `updates.svelte.ts`), qui si rispecchia solo lo
/// stato per la tray (`tray.rs`), che altrimenti non avrebbe modo di saperlo
/// senza duplicare tutta la logica lato frontend. `None` quando non c'è
/// nulla da segnalare.
#[derive(Default)]
pub struct UpdateState(Mutex<Option<String>>);

impl UpdateState {
    pub(crate) fn pending_version(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}

#[tauri::command]
pub fn report_update_available(state: tauri::State<'_, UpdateState>, version: Option<String>) {
    *state.0.lock().unwrap() = version;
}

const FIRST_CHECK_DELAY: std::time::Duration = std::time::Duration::from_secs(20);
const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// Controlla gli aggiornamenti dal backend, anche a finestra chiusa: senza
/// questo il badge nella tray comparirebbe solo dopo aver aperto la finestra
/// (il controllo vero e proprio, con la possibilità di installare, resta nel
/// frontend). Serve solo a far sapere alla tray che c'è una versione nuova.
/// Solo nelle build di rilascio: in sviluppo la versione dell'app non è
/// significativa e non si vuole toccare la rete.
#[cfg(not(debug_assertions))]
pub(crate) fn spawn_background_check(app: tauri::AppHandle) {
    use tauri::Manager;
    use tauri_plugin_updater::UpdaterExt;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            if let Ok(updater) = app.updater() {
                if let Ok(Some(update)) = updater.check().await {
                    *app.state::<UpdateState>().0.lock().unwrap() = Some(update.version);
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(debug_assertions)]
pub(crate) fn spawn_background_check(_app: tauri::AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_version_starts_empty_and_reflects_reports() {
        let state = UpdateState::default();
        assert_eq!(state.pending_version(), None);

        *state.0.lock().unwrap() = Some("1.2.3".to_string());
        assert_eq!(state.pending_version(), Some("1.2.3".to_string()));

        *state.0.lock().unwrap() = None;
        assert_eq!(state.pending_version(), None);
    }
}
