use std::sync::Mutex;

/// Ultima versione di aggiornamento nota, riportata dal frontend dopo un
/// controllo — il controllo vero e la vera installazione restano lato JS
/// (plugin updater di Tauri, `updates.svelte.ts`), qui si rispecchia solo lo
/// stato per la tray (`tray.rs`), che altrimenti non avrebbe modo di saperlo
/// senza duplicare tutta la logica di controllo/salta versione già gestita
/// lato frontend. `None` quando non c'è nulla da segnalare: nessun
/// aggiornamento trovato, oppure la versione trovata è stata esplicitamente
/// saltata dall'utente (il frontend segnala `null` in quel caso, vedi
/// `updates.svelte.ts::skipUpdate`) — un controllo manuale successivo la
/// rimanda comunque, indipendentemente da questo stato.
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
