use rand::distributions::Alphanumeric;
use rand::Rng;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Rappresenta l'istanza in background di `rclone rcd` usata da Rclone Easy
/// per tutte le operazioni verso rclone (mount/sync/bisync/test connessione).
/// `child` ha `kill_on_drop` attivo come rete di sicurezza, ma non basta da
/// solo: Tauri non garantisce che il Drop di questo stato giri alla chiusura
/// normale dell'app. La terminazione affidabile è quella esplicita in
/// `shutdown`, agganciata a `RunEvent::Exit` in `lib.rs`.
struct RcdProcess {
    child: Child,
    base_url: String,
    user: String,
    pass: String,
    client: reqwest::Client,
    /// `Some` se la config è protetta da password (`config_password.rs`) —
    /// tenuta qui in memoria per la durata della sessione così l'utente non
    /// deve reinserirla per cambiarla o rimuoverla: l'ha già data una volta
    /// per sbloccare l'app, non ha senso richiederla di nuovo nella stessa
    /// sessione. Mai scritta su disco da questa app (rclone stesso la
    /// legge solo dalla variabile d'ambiente che le passiamo all'avvio).
    config_password: Option<String>,
}

/// Stato gestito da Tauri. `Err` quando l'avvio del demone è fallito (es.
/// rclone non installato) — l'errore resta visibile ai comandi invece di
/// far fallire silenziosamente l'avvio dell'app. Usato anche per
/// rappresentare "in attesa della password" (vedi `LOCKED_SENTINEL`)
/// quando la config risulta protetta: stesso schema "Err = non ancora
/// utilizzabile", senza bisogno di un terzo stato dedicato.
pub struct RcdState(Mutex<Result<RcdProcess, String>>);

/// Valore speciale (mai un vero messaggio d'errore di rclone, che parla
/// sempre di connessioni/processi) usato come segnaposto in `RcdState`
/// finché la config protetta da password non viene sbloccata — distingue
/// "in attesa della password" (mostra la schermata di sblocco) da "rcd
/// davvero non disponibile" (mostra l'avviso generico), vedi `needs_unlock`.
const LOCKED_SENTINEL: &str = "__rclone_easy_config_locked__";

/// Stato iniziale quando la config risulta protetta da password
/// all'avvio dell'app: nessun demone avviato finché non arriva
/// `unlock` con la password giusta.
pub(crate) fn locked_state() -> RcdState {
    RcdState(Mutex::new(Err(LOCKED_SENTINEL.to_string())))
}

/// La config sotto `config_path` è protetta da password se inizia con il
/// commento che rclone stesso scrive in testa a una config cifrata (il
/// marker `RCLONE_ENCRYPT_V0:` segue su una riga successiva, non la prima)
/// — nessuna chiamata esterna necessaria, un file assente o illeggibile
/// conta come "non protetta", mai bloccare l'avvio per questo.
pub(crate) fn config_is_encrypted(config_path: &Path) -> bool {
    std::fs::read(config_path).map(|bytes| bytes.starts_with(b"# Encrypted rclone configuration File")).unwrap_or(false)
}

/// `true` se l'app è in attesa che l'utente sblocchi la config con la
/// password — la UI usa questo per decidere se mostrare la schermata di
/// sblocco invece della home.
pub(crate) async fn needs_unlock(state: &RcdState) -> bool {
    matches!(&*state.0.lock().await, Err(e) if e == LOCKED_SENTINEL)
}

/// Avvia il demone con la password fornita. Solo un successo sostituisce lo
/// stato segnaposto (`locked_state`) con il processo vero — un fallimento
/// (password sbagliata inclusa) lascia lo stato invariato, ancora "in
/// attesa di sblocco" (`needs_unlock` resta `true`), così la UI può
/// mostrare l'errore restituito qui e permettere un altro tentativo senza
/// dover riavviare l'app né perdere il segnaposto che la distingue da un
/// rcd genuinamente non disponibile.
pub(crate) async fn unlock(state: &RcdState, config_path: &Path, password: &str) -> Result<(), String> {
    match start_rcd_in(config_path, Some(password)).await {
        Ok(process) => {
            *state.0.lock().await = Ok(process);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Password attualmente in uso per la config, se protetta — `None` sia se
/// non è protetta sia se il demone non è (ancora) avviato. Usata da
/// `config_password.rs` per cambiare/rimuovere la protezione senza dover
/// richiedere di nuovo la password corrente all'utente.
pub(crate) async fn current_config_password(state: &RcdState) -> Option<String> {
    match &*state.0.lock().await {
        Ok(process) => process.config_password.clone(),
        Err(_) => None,
    }
}

/// Aggiorna la password ricordata in memoria dopo un cambio/rimozione
/// riuscita (`config_password.rs`) — il demone già in esecuzione non ha
/// bisogno di riavviare: ha la config già in memoria da quando è partito,
/// il file su disco cambia password "sotto di lui" senza che se ne accorga.
pub(crate) async fn update_remembered_password(state: &RcdState, new_password: Option<String>) {
    if let Ok(process) = &mut *state.0.lock().await {
        process.config_password = new_password;
    }
}

fn random_token(len: usize) -> String {
    rand::thread_rng().sample_iter(&Alphanumeric).take(len).map(char::from).collect()
}

fn pick_free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("impossibile riservare una porta locale: {e}"))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("impossibile leggere la porta riservata: {e}"))
}

/// Estrae il campo `version` dalla risposta JSON di `core/version`.
fn extract_version(body: &serde_json::Value) -> Result<String, String> {
    body.get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("campo 'version' mancante nella risposta di rclone rcd: {body}"))
}

/// POST generico verso un endpoint dell'RC API di rclone. Punto unico da cui
/// passano tutte le chiamate (avvio, status, e i comandi di `remotes.rs`),
/// così autenticazione e gestione errori restano coerenti in un solo posto.
async fn rc_call_with(
    client: &reqwest::Client,
    base_url: &str,
    user: &str,
    pass: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let response = client
        .post(format!("{base_url}/{path}"))
        .basic_auth(user, Some(pass))
        .json(body)
        .send()
        .await
        .map_err(|e| format!("richiesta a rclone rcd fallita: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(error_message_from_response(status, &text));
    }

    response.json().await.map_err(|e| format!("risposta non valida da rclone rcd: {e}"))
}

/// Estrae un messaggio d'errore leggibile dalla risposta di rclone. Le
/// risposte di errore dell'RC API sono un oggetto JSON con un campo
/// `"error"` già pulito e descrittivo (es. "InvalidAccessKeyId: ..."):
/// usarlo direttamente invece di mostrare l'intero corpo JSON grezzo con lo
/// stato HTTP in testa, che seppellisce il messaggio utile sotto rumore
/// tecnico. Ripiega su stato + testo grezzo per risposte che non sono nel
/// formato JSON atteso, es. errori di rete/proxy dove rclone stesso non ha
/// gestito la richiesta.
fn error_message_from_response(status: reqwest::StatusCode, body_text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body_text)
        .ok()
        .and_then(|body| body.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| format!("rclone rcd ha risposto con stato {status}: {body_text}"))
}

async fn rc_call(process: &RcdProcess, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
    rc_call_with(&process.client, &process.base_url, &process.user, &process.pass, path, body).await
}

/// Informazioni di connessione clonate fuori dal lock di `RcdState` (`reqwest::Client`
/// è economico da clonare, condivide internamente il pool di connessioni).
/// Serve per chiamate potenzialmente lunghe (es. l'attesa di un'autorizzazione
/// OAuth in `oauth_remote.rs`, che può restare bloccata minuti in attesa
/// dell'utente nel browser) che altrimenti, passando per `call` e tenendo il
/// lock per tutta la durata, congelerebbero ogni altro comando dell'app.
#[derive(Clone)]
pub(crate) struct ConnectionInfo {
    client: reqwest::Client,
    base_url: String,
    user: String,
    pass: String,
}

impl ConnectionInfo {
    pub(crate) async fn call(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
        rc_call_with(&self.client, &self.base_url, &self.user, &self.pass, path, &body).await
    }
}

/// Tiene il lock solo per il tempo di clonare le informazioni di
/// connessione, non per l'uso successivo — a differenza di `call`, pensata
/// per chiamate rapide dove tenere il lock per tutta la richiesta va bene.
pub(crate) async fn connection_info(state: &RcdState) -> Result<ConnectionInfo, String> {
    let guard = state.0.lock().await;
    match &*guard {
        Ok(process) => Ok(ConnectionInfo {
            client: process.client.clone(),
            base_url: process.base_url.clone(),
            user: process.user.clone(),
            pass: process.pass.clone(),
        }),
        Err(e) => Err(e.clone()),
    }
}

async fn query_version(process: &RcdProcess) -> Result<String, String> {
    let body = rc_call(process, "core/version", &serde_json::json!({})).await?;
    extract_version(&body)
}

/// Ripete la query finché rclone rcd non risponde o scade il timeout:
/// appena avviato il processo, il socket può impiegare qualche decina di ms
/// ad aprirsi.
async fn wait_until_ready(process: &RcdProcess) -> Result<String, String> {
    let mut last_err = String::new();
    for _ in 0..40 {
        match query_version(process).await {
            Ok(version) => return Ok(version),
            Err(e) => last_err = e,
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("rclone rcd non ha risposto entro il timeout di avvio: {last_err}"))
}

/// Solo nei test: `rclone rcd` apre **da solo** un browser via `xdg-open`
/// non appena il flusso OAuth (`oauth_remote.rs`) risponde `true` a
/// `config_is_local` — comportamento interno di rclone, non del nostro
/// `open_in_browser` (già no-op sotto `#[cfg(test)]`, ma senza effetto su
/// un sottoprocesso esterno). Anteponendo alla PATH del processo figlio una
/// cartella con un `xdg-open` finto (no-op) si neutralizza solo per il
/// sottoprocesso di test, senza toccare l'app reale (`cfg(test)`, non esiste
/// nel binario di produzione).
#[cfg(test)]
fn fake_xdg_open_path_prefix() -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join("rclone-easy-test-fake-bin");
    std::fs::create_dir_all(&dir).expect("impossibile creare la cartella per l'xdg-open finto");
    let script = dir.join("xdg-open");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").expect("impossibile scrivere l'xdg-open finto");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("impossibile rendere eseguibile l'xdg-open finto");
    dir
}

/// All'avvio, ripulisce eventuali `rclone rcd` orfani lasciati da
/// un'istanza precedente di Rclone Easy che non è riuscita a fare uno
/// shutdown pulito — `kill -9`, crash, spegnimento forzato del PC, o un
/// segnale non intercettabile (il gestore SIGTERM in `lib.rs` copre solo i
/// casi dove il segnale può essere intercettato).
///
/// Riconosciuti per corrispondenza esatta del percorso di config passato
/// con `--config`: solo un rcd avviato da QUESTA app (con QUESTA cartella
/// di configurazione) viene considerato, mai un rclone rcd generico
/// dell'utente per altri scopi. Solo Linux (legge `/proc`). Best-effort: un
/// fallimento qui (es. `/proc` non leggibile) non deve impedire l'avvio
/// dell'app.
///
/// Limite noto: se l'istanza precedente aveva un mount FUSE attivo al
/// momento del crash, questa pulizia termina comunque il suo rcd (non
/// abbiamo le sue credenziali RC per chiedergli uno smontaggio pulito,
/// perse insieme all'istanza che è morta) — il mountpoint può restare
/// "stale" finché non lo si smonta a mano (`fusermount -u`).
///
/// `/proc/<pid>/cmdline` è una sequenza di argomenti separati da byte NUL
/// (nessuno spazio) — questa funzione fa solo il parsing, separata dalla
/// lettura del file per poterla testare con dati sintetici invece che con
/// `/proc` reale.
fn parse_cmdline(bytes: &[u8]) -> Vec<String> {
    bytes.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

/// `true` se gli argomenti corrispondono a un `rclone rcd` avviato con
/// esattamente il nostro percorso di config — separata dalla scansione di
/// `/proc` per lo stesso motivo di `parse_cmdline`.
fn is_our_orphaned_rcd(args: &[String], config_path_str: &str) -> bool {
    let is_rcd =
        args.first().map(|a| a.ends_with("rclone")).unwrap_or(false) && args.get(1).map(String::as_str) == Some("rcd");
    let has_our_config = args.windows(2).any(|w| w[0] == "--config" && w[1] == config_path_str);
    is_rcd && has_our_config
}

#[cfg(all(target_os = "linux", not(test)))]
fn cleanup_orphaned_rcd_processes(config_path: &Path) {
    let Ok(entries) = std::fs::read_dir("/proc") else { return };
    let config_path_str = config_path.to_string_lossy().to_string();

    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else { continue };
        let args = parse_cmdline(&cmdline);

        if is_our_orphaned_rcd(&args, &config_path_str) {
            eprintln!("trovato un 'rclone rcd' orfano di una sessione precedente (pid {pid}), lo termino");
            let _ = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status();
        }
    }
}

/// Estrae i PID di eventuali `rclone.exe rcd` orfani dall'output JSON di
/// `Get-CimInstance Win32_Process` (PowerShell) — unico modo pratico su
/// Windows per leggere la riga di comando completa di un processo (a
/// differenza di `/proc/<pid>/cmdline` su Linux, `tasklist` da solo non la
/// mostra). Separata dalla chiamata PowerShell per poterla testare con dati
/// sintetici, stesso schema di `is_our_orphaned_rcd`. PowerShell restituisce
/// un oggetto singolo (non un array) quando c'è un solo risultato — entrambe
/// le forme vanno gestite. Riconosciuto per corrispondenza esatta del
/// percorso di config passato con `--config` (mai un rclone.exe generico
/// dell'utente per altri scopi), più la presenza di `rcd` come sottocomando.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn find_orphaned_rclone_pids_windows(cim_json: &str, config_path_str: &str) -> Vec<u32> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(cim_json) else { return Vec::new() };
    let entries: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(arr) => arr.iter().collect(),
        serde_json::Value::Object(_) => vec![&value],
        _ => Vec::new(),
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let pid = entry.get("ProcessId")?.as_u64()? as u32;
            let cmdline = entry.get("CommandLine")?.as_str()?;
            if cmdline.contains(" rcd ") && cmdline.contains(config_path_str) {
                Some(pid)
            } else {
                None
            }
        })
        .collect()
}

/// Equivalente Windows di `cleanup_orphaned_rcd_processes` sopra: chiudere
/// l'app (che resta in tray) o disinstallarla può lasciare `rclone.exe` in
/// esecuzione (nessun gestore SIGTERM/SIGINT equivalente su Windows, vedi
/// `spawn_signal_shutdown_handler` in `lib.rs`), bloccando poi un
/// reinstall/aggiornamento successivo ("Error opening file for writing" —
/// mitigato anche lato installer in `windows/hooks.nsh`, questo è il lato
/// runtime dello stesso problema). Best-effort, come la versione Linux: un
/// fallimento qui (PowerShell non disponibile, nessun processo trovato) non
/// deve impedire l'avvio dell'app.
#[cfg(all(target_os = "windows", not(test)))]
fn cleanup_orphaned_rcd_processes(config_path: &Path) {
    let config_path_str = config_path.to_string_lossy().to_string();
    let mut command = std::process::Command::new("powershell");
    command.args([
        "-NoProfile",
        "-Command",
        "Get-CimInstance Win32_Process -Filter \"Name='rclone.exe'\" | Select-Object ProcessId,CommandLine | ConvertTo-Json -Compress",
    ]);
    crate::rclone_bin::hide_console_window(&mut command);
    let Ok(output) = command.output() else { return };
    let Ok(text) = String::from_utf8(output.stdout) else { return };

    for pid in find_orphaned_rclone_pids_windows(&text, &config_path_str) {
        eprintln!("trovato un 'rclone rcd' orfano di una sessione precedente (pid {pid}), lo termino");
        let mut kill = std::process::Command::new("taskkill");
        kill.args(["/F", "/PID", &pid.to_string()]);
        crate::rclone_bin::hide_console_window(&mut kill);
        let _ = kill.status();
    }
}

/// `password`: se la config è protetta (vedi `config_password.rs`), la
/// passphrase da passare a rclone via `RCLONE_CONFIG_PASS`. Una password
/// sbagliata **non** fa fallire l'avvio del demone né `wait_until_ready`:
/// `core/version` risponde comunque, rclone non tenta di decifrare la
/// config finché non serve davvero. Per questo, quando è stata data una
/// password, dopo l'avvio si fa anche una `config/listremotes` (che quella
/// sì costringe rclone a decifrare) apposta per far emergere subito una
/// password sbagliata, invece di scoprirlo alla prima azione dell'utente
/// sui suoi remote.
async fn start_rcd_in(config_path: &Path, password: Option<&str>) -> Result<RcdProcess, String> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("impossibile creare la cartella di configurazione '{}': {e}", parent.display()))?;
    }

    #[cfg(any(all(target_os = "linux", not(test)), all(target_os = "windows", not(test))))]
    cleanup_orphaned_rcd_processes(config_path);

    let port = pick_free_port()?;
    let user = random_token(24);
    let pass = random_token(24);

    let mut command = Command::new(crate::rclone_bin::resolve_rclone_binary());
    command
        .args([
            "rcd",
            "--rc-addr",
            &format!("127.0.0.1:{port}"),
            "--rc-user",
            &user,
            "--rc-pass",
            &pass,
            "--config",
            &config_path.to_string_lossy(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    crate::rclone_bin::hide_console_window_tokio(&mut command);

    if let Some(password) = password {
        command.env("RCLONE_CONFIG_PASS", password);
    }

    // `rclone rcd` apre da sé il browser per l'autorizzazione OAuth via il
    // suo `xdg-open` interno — ereditando l'ambiente inquinato da `AppRun`
    // (bundling AppImage) soffre dello stesso fallimento silenzioso già
    // risolto per il file manager, vedi `open_external.rs`.
    #[cfg(all(target_os = "linux", not(test)))]
    {
        for var in crate::open_external::APPIMAGE_POLLUTED_ENV_VARS {
            command.env_remove(var);
        }
        command.env("PATH", crate::open_external::CLEAN_SYSTEM_PATH);
    }

    #[cfg(test)]
    {
        let fake_bin = fake_xdg_open_path_prefix();
        let existing_path = std::env::var("PATH").unwrap_or_default();
        command.env("PATH", format!("{}:{existing_path}", fake_bin.display()));
    }

    let child = command.spawn().map_err(|e| format!("impossibile avviare 'rclone rcd': {e}"))?;

    let mut process = RcdProcess {
        child,
        base_url: format!("http://127.0.0.1:{port}"),
        user,
        pass,
        client: reqwest::Client::new(),
        config_password: password.map(str::to_string),
    };

    wait_until_ready(&process).await?;

    if password.is_some() {
        if let Err(e) = rc_call(&process, "config/listremotes", &serde_json::json!({})).await {
            let _ = process.child.start_kill();
            let _ = process.child.wait().await;
            return Err(e);
        }
    }

    Ok(process)
}

/// Avvia il demone puntato sulla config propria dell'app (mai sul
/// `rclone.conf` di sistema) e produce lo stato da passare a
/// `tauri::Manager::manage`. Non propaga l'errore al chiamante: un
/// fallimento (es. rclone non installato) deve restare consultabile da
/// `rcd_status` invece di impedire l'avvio dell'app. Chiamata solo quando
/// la config non è protetta da password — altrimenti si parte da
/// `locked_state()` e si passa da `unlock`, vedi `lib.rs`.
pub async fn build_state(config_path: PathBuf) -> RcdState {
    RcdState(Mutex::new(start_rcd_in(&config_path, None).await))
}

/// Punto d'ingresso per gli altri moduli (es. `remotes.rs`) che devono
/// parlare con l'RC API senza conoscere i dettagli di autenticazione/porta.
pub(crate) async fn call(state: &RcdState, path: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    let guard = state.0.lock().await;
    match &*guard {
        Ok(process) => rc_call(process, path, &body).await,
        Err(e) => Err(e.clone()),
    }
}

#[tauri::command]
pub async fn rcd_status(state: tauri::State<'_, RcdState>) -> Result<String, String> {
    let body = call(&state, "core/version", serde_json::json!({})).await?;
    extract_version(&body)
}

/// Termina subito il demone rclone rcd, se attivo. Da chiamare esplicitamente
/// alla chiusura dell'app (`RunEvent::Exit`): vedi il commento su
/// `RcdProcess` sul perché non ci si può affidare solo a `kill_on_drop`.
/// Dopo `start_kill` serve anche `wait`: senza raccogliere il processo
/// terminato resta zombie (visibile in `/proc` finché non viene raccolto).
///
/// **Smontaggio prima del kill**: se un mount FUSE (`mounts.rs`) è attivo,
/// uno `start_kill` diretto lo lascia "stale": il mountpoint resta
/// agganciato nel kernel ma il processo che lo serviva è morto, risultato
/// "Transport endpoint not connected" finché non si smonta a mano con
/// `fusermount -u`. Il mount FUSE vive dentro lo stesso processo di
/// `rclone rcd`, quindi non ha modo di negoziare uno smontaggio pulito una
/// volta ucciso. Chiamare `mount/unmountall` mentre il demone è ancora vivo
/// e risponde evita il problema: best-effort, un errore qui (es. nessun
/// mount attivo) non deve impedire comunque la terminazione del processo.
pub async fn shutdown(state: &RcdState) {
    let mut guard = state.0.lock().await;
    if let Ok(process) = &mut *guard {
        let _ = rc_call(process, "mount/unmountall", &serde_json::json!({})).await;
        let _ = process.child.start_kill();
        let _ = process.child.wait().await;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct TempDir {
        pub(crate) path: PathBuf,
    }

    impl TempDir {
        pub(crate) fn new(label: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "rclone-easy-test-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()
            ));
            Self { path }
        }

        pub(crate) fn config_path(&self) -> PathBuf {
            self.path.join("rclone.conf")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn parse_cmdline_splits_on_nul_bytes_and_drops_empty_entries() {
        let raw = b"rclone\0rcd\0--config\0/home/utente/.config/RcloneEasy/rclone.conf\0";
        assert_eq!(
            parse_cmdline(raw),
            vec!["rclone", "rcd", "--config", "/home/utente/.config/RcloneEasy/rclone.conf"]
        );
    }

    #[test]
    fn is_our_orphaned_rcd_matches_our_own_config_path() {
        let args: Vec<String> = ["/home/utente/.local/bin/rclone", "rcd", "--rc-addr", "127.0.0.1:1234", "--config", "/a/rclone.conf"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(is_our_orphaned_rcd(&args, "/a/rclone.conf"));
    }

    #[test]
    fn is_our_orphaned_rcd_ignores_a_different_config_path() {
        let args: Vec<String> =
            ["rclone", "rcd", "--config", "/qualcun-altro/rclone.conf"].into_iter().map(String::from).collect();
        assert!(!is_our_orphaned_rcd(&args, "/a/rclone.conf"));
    }

    #[test]
    fn is_our_orphaned_rcd_ignores_a_non_rcd_rclone_invocation() {
        let args: Vec<String> = ["rclone", "version"].into_iter().map(String::from).collect();
        assert!(!is_our_orphaned_rcd(&args, "/a/rclone.conf"));
    }

    #[test]
    fn is_our_orphaned_rcd_ignores_an_unrelated_process_that_happens_to_mention_the_path() {
        let args: Vec<String> =
            ["cat", "/a/rclone.conf"].into_iter().map(String::from).collect();
        assert!(!is_our_orphaned_rcd(&args, "/a/rclone.conf"));
    }

    #[test]
    fn find_orphaned_rclone_pids_windows_matches_our_config_path_as_a_single_object() {
        let json = r#"{"ProcessId":1234,"CommandLine":"\"C:\\Users\\utente\\AppData\\Local\\rclone-easy\\rclone.exe\" rcd --rc-addr 127.0.0.1:9999 --config \"C:\\Users\\utente\\AppData\\Roaming\\RcloneEasy\\rclone.conf\""}"#;
        let pids = find_orphaned_rclone_pids_windows(json, r"C:\Users\utente\AppData\Roaming\RcloneEasy\rclone.conf");
        assert_eq!(pids, vec![1234]);
    }

    #[test]
    fn find_orphaned_rclone_pids_windows_matches_multiple_results_as_an_array() {
        let json = r#"[
            {"ProcessId":1,"CommandLine":"\"rclone.exe\" rcd --config \"C:\\a\\rclone.conf\""},
            {"ProcessId":2,"CommandLine":"\"rclone.exe\" rcd --config \"C:\\b\\rclone.conf\""}
        ]"#;
        assert_eq!(find_orphaned_rclone_pids_windows(json, r"C:\a\rclone.conf"), vec![1]);
    }

    #[test]
    fn find_orphaned_rclone_pids_windows_ignores_a_non_rcd_invocation() {
        let json = r#"{"ProcessId":5,"CommandLine":"\"rclone.exe\" version --config \"C:\\a\\rclone.conf\""}"#;
        assert!(find_orphaned_rclone_pids_windows(json, r"C:\a\rclone.conf").is_empty());
    }

    #[test]
    fn find_orphaned_rclone_pids_windows_handles_no_results() {
        assert!(find_orphaned_rclone_pids_windows("", r"C:\a\rclone.conf").is_empty());
        assert!(find_orphaned_rclone_pids_windows("null", r"C:\a\rclone.conf").is_empty());
    }

    #[test]
    fn random_token_has_requested_length_and_is_alphanumeric() {
        let token = random_token(24);
        assert_eq!(token.len(), 24);
        assert!(token.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn random_token_calls_produce_different_values() {
        assert_ne!(random_token(24), random_token(24));
    }

    #[test]
    fn pick_free_port_returns_a_nonzero_port() {
        let port = pick_free_port().unwrap();
        assert_ne!(port, 0);
    }

    #[test]
    fn extract_version_reads_the_version_field() {
        let body = serde_json::json!({"version": "v1.75.0"});
        assert_eq!(extract_version(&body).unwrap(), "v1.75.0");
    }

    #[test]
    fn extract_version_fails_when_field_missing() {
        let body = serde_json::json!({"other": "value"});
        assert!(extract_version(&body).is_err());
    }

    #[test]
    fn error_message_from_response_extracts_the_error_field() {
        let body = r#"{"error":"api error InvalidAccessKeyId: The AWS Access Key Id you provided does not exist in our records.","input":{},"path":"operations/list","status":500}"#;
        assert_eq!(
            error_message_from_response(reqwest::StatusCode::INTERNAL_SERVER_ERROR, body),
            "api error InvalidAccessKeyId: The AWS Access Key Id you provided does not exist in our records."
        );
    }

    #[test]
    fn error_message_from_response_falls_back_to_raw_text_for_non_json_body() {
        let message = error_message_from_response(reqwest::StatusCode::BAD_GATEWAY, "<html>Bad Gateway</html>");
        assert!(message.contains("502"));
        assert!(message.contains("<html>Bad Gateway</html>"));
    }

    #[test]
    fn error_message_from_response_falls_back_when_error_field_missing() {
        let message =
            error_message_from_response(reqwest::StatusCode::INTERNAL_SERVER_ERROR, r#"{"other":"value"}"#);
        assert!(message.contains("500"));
        assert!(message.contains(r#"{"other":"value"}"#));
    }

    #[tokio::test]
    async fn start_rcd_spawns_a_working_daemon_and_kills_it_on_drop() {
        // Richiede rclone installato nell'ambiente di test.
        let dir = TempDir::new("kill-on-drop");
        let process =
            start_rcd_in(&dir.config_path(), None).await.expect("rclone rcd dovrebbe avviarsi correttamente");
        let version = query_version(&process).await.expect("il demone dovrebbe rispondere");
        assert!(version.starts_with('v'));

        let pid = process.child.id().expect("il processo dovrebbe avere un pid mentre è vivo");
        drop(process);

        // kill_on_drop è asincrono lato OS: diamo un attimo di margine prima
        // di verificare che il processo non esista più.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let still_alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
        assert!(!still_alive, "il processo rclone rcd dovrebbe essere terminato dopo il drop");
    }

    /// Regressione per il bug descritto nel commento su `shutdown`: senza lo
    /// smontaggio esplicito, la cartella di mount resterebbe "stale"
    /// (ENOTCONN) dopo il kill invece di tornare una normale cartella
    /// locale vuota.
    #[tokio::test]
    async fn shutdown_cleanly_unmounts_active_mounts_before_killing_the_daemon() {
        let rcd_dir = TempDir::new("shutdown-mount-rcd");
        let source_dir = TempDir::new("shutdown-mount-source");
        let mount_dir = TempDir::new("shutdown-mount-mnt");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&mount_dir.path).unwrap();
        std::fs::write(source_dir.path.join("prova.txt"), "contenuto").unwrap();

        let process = start_rcd_in(&rcd_dir.config_path(), None).await.expect("rclone rcd dovrebbe avviarsi correttamente");
        rc_call(
            &process,
            "mount/mount",
            &serde_json::json!({ "fs": source_dir.path.to_string_lossy(), "mountPoint": mount_dir.path.to_string_lossy() }),
        )
        .await
        .expect("il mount dovrebbe riuscire");

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(mount_dir.path.join("prova.txt").exists(), "il mount dovrebbe essere attivo prima dello shutdown");

        let pid = process.child.id().expect("il processo dovrebbe avere un pid mentre è vivo");
        let state = RcdState(Mutex::new(Ok(process)));

        shutdown(&state).await;

        tokio::time::sleep(Duration::from_millis(300)).await;
        let still_alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
        assert!(!still_alive, "il processo rclone rcd dovrebbe essere terminato dopo shutdown");

        let entries: Vec<_> = std::fs::read_dir(&mount_dir.path)
            .expect("la cartella dovrebbe essere leggibile normalmente dopo uno smontaggio pulito, non 'stale'")
            .collect();
        assert!(entries.is_empty(), "la cartella di mount dovrebbe essere tornata una cartella locale vuota");
    }

    #[tokio::test]
    async fn shutdown_kills_the_running_daemon() {
        let dir = TempDir::new("shutdown");
        let process =
            start_rcd_in(&dir.config_path(), None).await.expect("rclone rcd dovrebbe avviarsi correttamente");
        let pid = process.child.id().expect("il processo dovrebbe avere un pid mentre è vivo");
        let state = RcdState(Mutex::new(Ok(process)));

        shutdown(&state).await;

        tokio::time::sleep(Duration::from_millis(200)).await;
        let still_alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
        assert!(!still_alive, "il processo rclone rcd dovrebbe essere terminato dopo shutdown");
    }

    #[tokio::test]
    async fn rcd_status_reports_the_stored_error_when_startup_failed() {
        let state = RcdState(Mutex::new(Err("rclone non trovato".to_string())));
        let guard = state.0.lock().await;
        match &*guard {
            Err(e) => assert_eq!(e, "rclone non trovato"),
            Ok(_) => panic!("atteso stato di errore"),
        }
    }

    #[tokio::test]
    async fn start_rcd_uses_the_given_config_path_not_the_system_one() {
        let dir = TempDir::new("own-config");
        let config_path = dir.config_path();
        let process = start_rcd_in(&config_path, None).await.expect("rclone rcd dovrebbe avviarsi correttamente");

        let body = rc_call(&process, "config/paths", &serde_json::json!({}))
            .await
            .expect("config/paths dovrebbe rispondere");
        let reported_path = body.get("config").and_then(|v| v.as_str()).expect("campo 'config' atteso");

        assert_eq!(std::path::Path::new(reported_path), config_path);
    }

    #[tokio::test]
    async fn connection_info_releases_the_lock_immediately_and_can_still_call_rc() {
        let dir = TempDir::new("connection-info");
        let state = build_state(dir.config_path()).await;

        let info = connection_info(&state).await.expect("connection_info dovrebbe riuscire");

        // Se connection_info tenesse ancora il lock, try_lock fallirebbe qui.
        assert!(state.0.try_lock().is_ok(), "il lock dovrebbe essere già libero dopo connection_info");

        let body = info.call("core/version", serde_json::json!({})).await.expect("la chiamata dovrebbe riuscire");
        assert!(extract_version(&body).unwrap().starts_with('v'));
    }
}
