use crate::rcd::RcdState;
use std::path::Path;
use tauri::{AppHandle, Manager};

/// Argomento speciale intercettato da `lib.rs::run_or_password_helper`
/// prima di avviare l'app Tauri vera e propria: quando rclone invoca
/// questo stesso binario come `--password-command` (vedi
/// `run_config_encryption` sotto), risponde stampando su stdout la
/// password giusta ed esce subito. Evita di dover mai scrivere una
/// password su un file temporaneo su disco solo per passarla a un
/// sottoprocesso — vive solo in variabili d'ambiente, ereditate dal
/// processo rclone che richiama questo binario.
///
/// Serve distinguere DUE password possibili, non una sola: `rclone
/// config encryption set` chiama `--password-command` fino a due volte
/// nella stessa invocazione — una per aprire una config già cifrata
/// esistente (senza `RCLONE_PASSWORD_CHANGE` impostata), una per la
/// password nuova (con quella variabile a "1", impostata da rclone
/// stesso) — verificato a mano contro un demone rclone reale: passare la
/// password nuova a entrambe le richieste fa fallire la prima con
/// "wrong password" perché prova a decifrare la config esistente con
/// quella sbagliata.
pub(crate) const PASSWORD_HELPER_ARG: &str = "--rclone-easy-print-password";
const HELPER_CURRENT_PASSWORD_ENV: &str = "RCLONE_EASY_HELPER_CURRENT_PASSWORD";
const HELPER_NEW_PASSWORD_ENV: &str = "RCLONE_EASY_HELPER_NEW_PASSWORD";

/// Risponde a un'invocazione di `--password-command` — usata sia dal vero
/// binario (`lib.rs::run_or_password_helper`) sia dai test.
pub(crate) fn print_helper_password() {
    let is_change = std::env::var("RCLONE_PASSWORD_CHANGE").as_deref() == Ok("1");
    let var = if is_change { HELPER_NEW_PASSWORD_ENV } else { HELPER_CURRENT_PASSWORD_ENV };
    if let Ok(password) = std::env::var(var) {
        print!("{password}");
    }
}

fn app_config_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, String> {
    app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))
}

/// Esegue `rclone config encryption <set|remove>` come sottoprocesso
/// separato dal demone `rcd` già in esecuzione — l'unico modo disponibile:
/// la RC API di rclone non espone questa funzionalità (verificato,
/// confermato anche dal maintainer di rclone). Sicuro farlo mentre `rcd` è
/// vivo: il demone ha già la config in memoria da quando è partito e non
/// la rilegge per le normali operazioni, quindi non nota il cambio sotto
/// di sé — non serve fermarlo né riavviarlo dopo.
///
/// `current_password`/`new_password` arrivano entrambe a `--password-command`
/// (vedi `PASSWORD_HELPER_ARG` sopra sul perché non basta `RCLONE_CONFIG_PASS`
/// da solo): assente = "set" per la prima volta, senza nulla da aprire.
/// `password_helper_exe`: normalmente questo stesso binario (`current_exe()`,
/// vedi i comandi sotto), separato a parametro solo per poterlo sostituire
/// nei test con uno script fittizio — `current_exe()` in `cargo test`
/// restituisce l'eseguibile dei test, che non ha l'intercettazione di
/// `PASSWORD_HELPER_ARG` fatta in `lib.rs::run_or_password_helper` (quella
/// vive nel binario `main.rs`, non nella libreria).
fn run_config_encryption(
    subcommand: &str,
    config_path: &Path,
    current_password: Option<&str>,
    new_password: Option<&str>,
    password_helper_exe: &Path,
) -> Result<(), String> {
    let mut command = std::process::Command::new(crate::rclone_bin::resolve_rclone_binary());
    command.args(["config", "encryption", subcommand, "--config", &config_path.to_string_lossy()]);

    if current_password.is_some() || new_password.is_some() {
        command.args(["--password-command", &format!("{} {PASSWORD_HELPER_ARG}", password_helper_exe.to_string_lossy())]);
    }
    if let Some(current) = current_password {
        command.env(HELPER_CURRENT_PASSWORD_ENV, current);
    }
    if let Some(new) = new_password {
        command.env(HELPER_NEW_PASSWORD_ENV, new);
    }

    let output = command.output().map_err(|e| format!("impossibile eseguire rclone: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("rclone ha rifiutato l'operazione: {}", stderr.trim()))
    }
}

/// rclone segnala una password sbagliata con un messaggio tecnico (spesso
/// un panic Go completo, es. "panic received: fatal error: Failed to load
/// config file ... unable to decrypt configuration" da `config/listremotes`
/// — la formulazione esatta varia, osservate sia "couldn't decrypt" che
/// "unable to decrypt", quindi si riconosce con un match generico su
/// "decrypt" invece di una frase fissa) illeggibile per un utente finale —
/// qui lo si riconosce e si sostituisce con un messaggio chiaro; altri
/// errori (es. rclone non installato) passano invariati, non sono questo
/// caso.
fn friendly_error(raw: String) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("wrong password") || lower.contains("decrypt") {
        "La password inserita non è corretta".to_string()
    } else {
        raw
    }
}

/// `true` se la config è già protetta da password — la UI usa questo per
/// decidere se mostrare "Imposta password" o "Cambia password"/"Rimuovi".
#[tauri::command]
pub async fn config_password_status(state: tauri::State<'_, RcdState>) -> Result<bool, String> {
    Ok(crate::rcd::current_config_password(&state).await.is_some())
}

/// Imposta una password per la prima volta, o la cambia se ce n'è già una
/// — stesso comando rclone in entrambi i casi, la differenza è solo se
/// forniamo o no la password corrente per aprire la config esistente.
/// `current_password` arriva dalla UI (campo "Password attuale", mostrato
/// solo quando la config è già protetta): richiesta esplicita di Simone,
/// niente più auto-compilazione dalla password ricordata in sessione, per
/// evitare che chiunque acceda all'app già sbloccata possa cambiare la
/// password senza conoscere quella attuale.
#[tauri::command]
pub async fn set_config_password(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    current_password: Option<String>,
    new_password: String,
) -> Result<(), String> {
    let config_path = app_config_dir(&app)?.join("rclone.conf");
    let exe = std::env::current_exe().map_err(|e| format!("impossibile determinare il percorso di questa applicazione: {e}"))?;
    run_config_encryption("set", &config_path, current_password.as_deref(), Some(&new_password), &exe).map_err(friendly_error)?;
    crate::rcd::update_remembered_password(&state, Some(new_password)).await;
    Ok(())
}

/// Rimuove la protezione — `current_password` inserita dalla UI proprio
/// prima di questa chiamata, stesso motivo di `set_config_password` sopra.
#[tauri::command]
pub async fn remove_config_password(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    current_password: String,
) -> Result<(), String> {
    let config_path = app_config_dir(&app)?.join("rclone.conf");
    let exe = std::env::current_exe().map_err(|e| format!("impossibile determinare il percorso di questa applicazione: {e}"))?;
    run_config_encryption("remove", &config_path, Some(&current_password), None, &exe).map_err(friendly_error)?;
    crate::rcd::update_remembered_password(&state, None).await;
    Ok(())
}

/// Interrogata dalla UI all'avvio (prima di mostrare la home) per decidere
/// se mostrare la schermata di sblocco invece del resto dell'app.
#[tauri::command]
pub async fn needs_unlock(state: tauri::State<'_, RcdState>) -> Result<bool, String> {
    Ok(crate::rcd::needs_unlock(&state).await)
}

/// Sblocca la config con la password inserita dall'utente nella schermata
/// di sblocco — un errore (password sbagliata inclusa) resta mostrabile
/// senza dover riavviare l'app, vedi `rcd::unlock`.
#[tauri::command]
pub async fn unlock_config(app: AppHandle, state: tauri::State<'_, RcdState>, password: String) -> Result<(), String> {
    let config_path = app_config_dir(&app)?.join("rclone.conf");
    crate::rcd::unlock(&state, &config_path, &password).await.map_err(friendly_error)?;
    // I mount con auto_mount configurato dovevano montarsi da soli
    // all'avvio (lib.rs::setup), ma quel tentativo è girato mentre la
    // config era ancora bloccata ed è fallito in silenzio contro lo stato
    // segnaposto — ora che il demone è vivo davvero, si ritenta.
    let config_dir = app_config_dir(&app)?;
    crate::mounts::auto_mount_all(&state, &config_dir).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;
    use std::path::PathBuf;

    /// Sostituto di `current_exe()` per i test (vedi il commento su
    /// `run_config_encryption`): uno script che stampa la variabile
    /// d'ambiente usata dal vero helper, eseguibile al posto del binario
    /// dell'app.
    fn fake_password_helper() -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("rclone-easy-test-fake-password-helper");
        std::fs::create_dir_all(&dir).expect("impossibile creare la cartella per l'helper finto");
        let script = dir.join("helper.sh");
        let content = format!(
            "#!/bin/sh\nif [ \"$RCLONE_PASSWORD_CHANGE\" = \"1\" ]; then printf '%s' \"${HELPER_NEW_PASSWORD_ENV}\"; else printf '%s' \"${HELPER_CURRENT_PASSWORD_ENV}\"; fi\n"
        );
        std::fs::write(&script, content).expect("impossibile scrivere l'helper finto");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("impossibile rendere eseguibile l'helper finto");
        script
    }

    #[tokio::test]
    async fn set_then_unlock_round_trips_with_the_right_password_and_rejects_the_wrong_one() {
        let dir = TempDir::new("config-password-set");
        let config_path = dir.config_path();
        std::fs::create_dir_all(&dir.path).unwrap();
        std::fs::write(&config_path, "").unwrap();
        let helper = fake_password_helper();

        assert!(!crate::rcd::config_is_encrypted(&config_path), "una config vuota non è cifrata");

        run_config_encryption("set", &config_path, None, Some("password-corretta"), &helper)
            .expect("impostare una password per la prima volta dovrebbe riuscire");

        assert!(crate::rcd::config_is_encrypted(&config_path), "dopo 'set' la config deve risultare cifrata");

        let state = crate::rcd::locked_state();
        let wrong = crate::rcd::unlock(&state, &config_path, "password-sbagliata").await;
        assert!(wrong.is_err(), "una password sbagliata deve fallire");
        assert!(crate::rcd::needs_unlock(&state).await, "resta bloccato dopo un tentativo fallito");

        let right = crate::rcd::unlock(&state, &config_path, "password-corretta").await;
        assert!(right.is_ok(), "la password giusta dovrebbe sbloccare: {right:?}");
        assert!(!crate::rcd::needs_unlock(&state).await, "non più bloccato dopo lo sblocco");
        assert_eq!(crate::rcd::current_config_password(&state).await.as_deref(), Some("password-corretta"));

        crate::rcd::shutdown(&state).await;
    }

    #[tokio::test]
    async fn change_then_remove_password_both_work_with_the_remembered_current_password() {
        let dir = TempDir::new("config-password-change");
        let config_path = dir.config_path();
        std::fs::create_dir_all(&dir.path).unwrap();
        std::fs::write(&config_path, "").unwrap();
        let helper = fake_password_helper();

        run_config_encryption("set", &config_path, None, Some("prima-password"), &helper).unwrap();

        // Cambio: serve la password corrente per aprire la config esistente.
        run_config_encryption("set", &config_path, Some("prima-password"), Some("seconda-password"), &helper)
            .expect("cambiare la password dovrebbe riuscire conoscendo quella corrente");

        let state = crate::rcd::locked_state();
        let with_old = crate::rcd::unlock(&state, &config_path, "prima-password").await;
        assert!(with_old.is_err(), "la vecchia password non deve più funzionare dopo il cambio");
        let with_new = crate::rcd::unlock(&state, &config_path, "seconda-password").await;
        assert!(with_new.is_ok(), "la nuova password dovrebbe sbloccare: {with_new:?}");
        crate::rcd::shutdown(&state).await;

        // Rimozione.
        run_config_encryption("remove", &config_path, Some("seconda-password"), None, &helper)
            .expect("rimuovere la password dovrebbe riuscire");
        assert!(!crate::rcd::config_is_encrypted(&config_path), "dopo 'remove' la config non deve più risultare cifrata");
    }
}
