use crate::rcd::{self, RcdState};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    pub success: bool,
    pub message: String,
    pub when_unix: u64,
}

/// Job di sincronizzazione monodirezionale (`rclone sync`, via RC
/// `sync/sync`) — un concetto proprio di Rclone Easy, non di rclone stesso
/// (che non ha "job salvati" nel suo config, solo remote). `source`/
/// `destination` sono già nel formato che `sync/sync` si aspetta: un
/// percorso locale assoluto, oppure `remoto:percorso` — costruite dal
/// frontend, non servono altre strutture intermedie.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncJob {
    pub name: String,
    pub source: String,
    pub destination: String,
    /// `Some(n)` = eseguito da solo ogni `n` minuti dal ciclo interno
    /// dell'app (`scheduler.rs`), oltre al normale "Esegui ora" manuale.
    /// `None` = solo manuale, comportamento di sempre.
    #[serde(default)]
    pub auto_interval_minutes: Option<u32>,
    /// `false` (default) = usa `sync/copy`: aggiorna/aggiunge file nella
    /// destinazione ma non cancella mai nulla lì, anche se sparisce dalla
    /// sorgente — comportamento sicuro di default, pensato apposta per non
    /// rischiare di svuotare la destinazione se la sorgente risulta vuota o
    /// temporaneamente irraggiungibile (es. un disco esterno scollegato: la
    /// cartella locale esiste ma è vuota, e un mirror la interpreterebbe
    /// come "tutto cancellato"). `true` = usa `sync/sync` (mirror reale,
    /// cancella anche nella destinazione) — richiede conferma esplicita
    /// dell'utente nel frontend prima di essere attivato, vedi il modal di
    /// avviso in RemoteRow.svelte.
    #[serde(default)]
    pub propagate_deletions: bool,
    /// Le ultime esecuzioni, più recente in testa (indice 0), limitate a
    /// `HISTORY_LIMIT` — non solo l'ultimo esito, per poter mostrare un
    /// log consultabile nella UI. Un `jobs.toml` scritto da una versione
    /// precedente (che aveva solo `lastRun`) riparte semplicemente con lo
    /// storico vuoto: `#[serde(default)]` ignora il campo non più
    /// riconosciuto invece di fallire la lettura.
    #[serde(default)]
    pub history: Vec<LastRun>,
}

/// `SyncJob` unito allo stato live "in esecuzione ora" — stesso principio
/// di `mounts::MountEntry` rispetto a `MountConfig`: `is_running` non è mai
/// salvato su disco (rispecchia solo `running_jobs()` al momento della
/// lettura), serve al frontend per mostrare "in corso" invece del
/// countdown alla prossima esecuzione mentre il job sta girando davvero.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncJobEntry {
    pub name: String,
    pub source: String,
    pub destination: String,
    pub auto_interval_minutes: Option<u32>,
    pub propagate_deletions: bool,
    pub history: Vec<LastRun>,
    pub is_running: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct JobsFile {
    #[serde(default)]
    jobs: Vec<SyncJob>,
}

fn jobs_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("jobs.toml")
}

pub(crate) fn load_from_dir(config_dir: &Path) -> Result<Vec<SyncJob>, String> {
    let path = jobs_file_path(config_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("impossibile leggere '{}': {e}", path.display()))?;
    let file: JobsFile = toml::from_str(&content).map_err(|e| format!("file dei job non valido: {e}"))?;
    Ok(file.jobs)
}

fn save_to_dir(config_dir: &Path, jobs: &[SyncJob]) -> Result<(), String> {
    std::fs::create_dir_all(config_dir)
        .map_err(|e| format!("impossibile creare '{}': {e}", config_dir.display()))?;
    let file = JobsFile { jobs: jobs.to_vec() };
    let content = toml::to_string_pretty(&file).map_err(|e| format!("impossibile serializzare i job: {e}"))?;
    let path = jobs_file_path(config_dir);
    std::fs::write(&path, content).map_err(|e| format!("impossibile scrivere '{}': {e}", path.display()))
}

/// Nome del remote referenziato da una stringa `fs` (`remoto:percorso`),
/// `None` se è un percorso locale — riconosciuto dal `/` iniziale (tutti i
/// percorsi locali scelti dal selettore di cartelle sono assoluti, nessun
/// nome di remote rclone può iniziare per `/`).
fn remote_name_of(fs: &str) -> Option<&str> {
    if fs.starts_with('/') {
        return None;
    }
    fs.split_once(':').map(|(name, _)| name)
}

fn job_remote_names(job: &SyncJob) -> [Option<&str>; 2] {
    [remote_name_of(&job.source), remote_name_of(&job.destination)]
}

/// Rifiuta un secondo backup per un remote già usato da un altro backup
/// (in sorgente o destinazione) — al massimo un backup per remote, stessa
/// semplificazione già applicata ai mount. `excluding` è il nome della voce
/// in corso di modifica, da non contare come "già esistente".
fn ensure_remotes_not_already_backed_up(
    jobs: &[SyncJob],
    source: &str,
    destination: &str,
    excluding: Option<&str>,
) -> Result<(), String> {
    for candidate in [remote_name_of(source), remote_name_of(destination)].into_iter().flatten() {
        let clash = jobs
            .iter()
            .filter(|j| excluding != Some(j.name.as_str()))
            .any(|j| job_remote_names(j).contains(&Some(candidate)));
        if clash {
            return Err(format!("esiste già un backup per '{candidate}': modificalo invece di crearne un altro"));
        }
    }
    Ok(())
}

pub(crate) fn create_job_in(
    config_dir: &Path,
    name: &str,
    source: &str,
    destination: &str,
    auto_interval_minutes: Option<u32>,
    propagate_deletions: bool,
) -> Result<(), String> {
    let mut jobs = load_from_dir(config_dir)?;
    if jobs.iter().any(|j| j.name == name) {
        return Err(format!("esiste già un job chiamato '{name}': scegli un altro nome"));
    }
    ensure_remotes_not_already_backed_up(&jobs, source, destination, None)?;
    jobs.push(SyncJob {
        name: name.to_string(),
        source: source.to_string(),
        destination: destination.to_string(),
        auto_interval_minutes,
        propagate_deletions,
        history: Vec::new(),
    });
    save_to_dir(config_dir, &jobs)
}

/// `old_name` individua il job da modificare, `name` è il nome (nuovo o
/// invariato) dopo la modifica — permette di rinominare insieme alle altre
/// modifiche in un solo salvataggio, stesso schema di `mounts::update_mount_in`.
fn update_job_in(
    config_dir: &Path,
    old_name: &str,
    name: &str,
    source: &str,
    destination: &str,
    auto_interval_minutes: Option<u32>,
    propagate_deletions: bool,
) -> Result<(), String> {
    let mut jobs = load_from_dir(config_dir)?;
    if name != old_name && jobs.iter().any(|j| j.name == name) {
        return Err(format!("esiste già un job chiamato '{name}': scegli un altro nome"));
    }
    ensure_remotes_not_already_backed_up(&jobs, source, destination, Some(old_name))?;
    let job = jobs.iter_mut().find(|j| j.name == old_name).ok_or_else(|| format!("nessun job chiamato '{old_name}'"))?;
    job.name = name.to_string();
    job.source = source.to_string();
    job.destination = destination.to_string();
    job.auto_interval_minutes = auto_interval_minutes;
    job.propagate_deletions = propagate_deletions;
    save_to_dir(config_dir, &jobs)
}

pub(crate) fn delete_job_in(config_dir: &Path, name: &str) -> Result<(), String> {
    let mut jobs = load_from_dir(config_dir)?;
    let before = jobs.len();
    jobs.retain(|j| j.name != name);
    if jobs.len() == before {
        return Err(format!("nessun job chiamato '{name}'"));
    }
    save_to_dir(config_dir, &jobs)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Quante esecuzioni passate tenere nello storico — abbastanza per un log
/// utile da consultare senza far crescere `jobs.toml` all'infinito.
const HISTORY_LIMIT: usize = 20;

/// Aggiunge in testa (più recente per prima) e tronca oltre `HISTORY_LIMIT`.
pub(crate) fn push_history<T>(history: &mut Vec<T>, entry: T) {
    history.insert(0, entry);
    history.truncate(HISTORY_LIMIT);
}

/// Esegue `sync/copy` o `sync/sync` in modalità asincrona (`_async: true`) e
/// segue l'esito con `job/status` finché non è `finished` — una sync può
/// durare a lungo (trasferimenti grossi), va lanciata così invece che in
/// modo sincrono. Usa `rcd::connection_info`, non `rcd::call`: tenere il
/// lock su `RcdState` per tutta l'attesa congelerebbe ogni altro comando
/// dell'app, stessa cautela già presa per l'attesa OAuth (`oauth_remote.rs`).
///
/// `propagate_deletions` sceglie l'endpoint: `sync/sync` (mirror, cancella
/// nella destinazione ciò che non c'è più nella sorgente) solo se
/// esplicitamente attivato dall'utente; `sync/copy` (mai cancella) è il
/// default sicuro — vedi il commento su `SyncJob::propagate_deletions` per
/// il motivo (una sorgente vuota per errore/disco scollegato non deve
/// poter svuotare la destinazione).
async fn execute_sync(state: &RcdState, source: &str, destination: &str, propagate_deletions: bool) -> Result<(), String> {
    let info = rcd::connection_info(state).await?;

    let endpoint = if propagate_deletions { "sync/sync" } else { "sync/copy" };
    let created =
        info.call(endpoint, serde_json::json!({ "srcFs": source, "dstFs": destination, "_async": true })).await?;
    let job_id = created
        .get("jobid")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("campo 'jobid' mancante nella risposta di rclone rcd: {created}"))?;

    loop {
        let status = info.call("job/status", serde_json::json!({ "jobid": job_id })).await?;
        let finished = status.get("finished").and_then(|v| v.as_bool()).unwrap_or(false);
        if finished {
            let success = status.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
            if success {
                return Ok(());
            }
            let error = status
                .get("error")
                .and_then(|v| v.as_str())
                .filter(|e| !e.is_empty())
                .unwrap_or("errore sconosciuto durante la sincronizzazione");
            return Err(error.to_string());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Nomi dei job attualmente in esecuzione — evita che lo scheduler interno
/// (`scheduler.rs`) rilanci un job il cui giro precedente non è ancora
/// finito (una sync grossa può durare più dell'intervallo configurato), e
/// che un click manuale su "Esegui ora" parta in parallelo a un'esecuzione
/// automatica già in corso dello stesso job.
fn running_jobs() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static RUNNING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    RUNNING.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Usata da `tray.rs` per decidere se mostrare l'icona "in esecuzione" —
/// riusa lo stesso insieme già mantenuto per evitare sovrapposizioni,
/// niente da tracciare in più.
pub(crate) fn any_job_running() -> bool {
    !running_jobs().lock().unwrap().is_empty()
}

/// Usata da `list_jobs` per popolare `SyncJobEntry::is_running`.
fn is_job_running(name: &str) -> bool {
    running_jobs().lock().unwrap().contains(name)
}

/// Esegue il job per nome e salva l'esito (successo o messaggio d'errore)
/// su `jobs.toml`, così l'ultimo risultato resta visibile anche dopo un
/// riavvio dell'app. Usata sia da "Esegui ora" (comando Tauri) sia dal
/// ciclo automatico di `scheduler.rs`.
pub(crate) async fn run_job_by_name(config_dir: &Path, state: &RcdState, name: &str) -> Result<(), String> {
    if !running_jobs().lock().unwrap().insert(name.to_string()) {
        return Err(format!("il job '{name}' è già in esecuzione"));
    }

    let mut jobs = match load_from_dir(config_dir) {
        Ok(jobs) => jobs,
        Err(e) => {
            running_jobs().lock().unwrap().remove(name);
            return Err(e);
        }
    };
    let job = match jobs.iter().find(|j| j.name == name).cloned() {
        Some(job) => job,
        None => {
            running_jobs().lock().unwrap().remove(name);
            return Err(format!("nessun job chiamato '{name}'"));
        }
    };

    let result = execute_sync(state, &job.source, &job.destination, job.propagate_deletions).await;

    let last_run = match &result {
        Ok(()) => LastRun { success: true, message: String::new(), when_unix: now_unix() },
        Err(e) => LastRun { success: false, message: e.clone(), when_unix: now_unix() },
    };
    if let Some(stored) = jobs.iter_mut().find(|j| j.name == name) {
        push_history(&mut stored.history, last_run);
    }
    let save_result = save_to_dir(config_dir, &jobs);

    running_jobs().lock().unwrap().remove(name);

    save_result?;
    result
}

fn app_config_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))
}

#[tauri::command]
pub fn list_jobs(app: AppHandle) -> Result<Vec<SyncJobEntry>, String> {
    let jobs = load_from_dir(&app_config_dir(&app)?)?;
    Ok(jobs
        .into_iter()
        .map(|j| SyncJobEntry {
            is_running: is_job_running(&j.name),
            name: j.name,
            source: j.source,
            destination: j.destination,
            auto_interval_minutes: j.auto_interval_minutes,
            propagate_deletions: j.propagate_deletions,
            history: j.history,
        })
        .collect())
}

/// Se `auto_interval_minutes` è impostato (il backup diventa "attivo",
/// vedi `activity.rs`), rifiuta se un altro servizio è già attivo per uno
/// dei remote coinvolti — un backup solo manuale non ha questa
/// restrizione, resta sempre possibile crearne/modificarne uno.
async fn ensure_activation_allowed(
    state: &RcdState,
    config_dir: &Path,
    source: &str,
    destination: &str,
    auto_interval_minutes: Option<u32>,
) -> Result<(), String> {
    if auto_interval_minutes.is_none() {
        return Ok(());
    }
    for candidate in [remote_name_of(source), remote_name_of(destination)].into_iter().flatten() {
        crate::activity::ensure_no_other_active_service(state, config_dir, candidate, crate::activity::ServiceKind::Backup)
            .await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn create_job(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    name: String,
    source: String,
    destination: String,
    auto_interval_minutes: Option<u32>,
    propagate_deletions: bool,
) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    ensure_activation_allowed(&state, &config_dir, &source, &destination, auto_interval_minutes).await?;
    create_job_in(&config_dir, &name, &source, &destination, auto_interval_minutes, propagate_deletions)
}

#[tauri::command]
pub async fn update_job(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    old_name: String,
    name: String,
    source: String,
    destination: String,
    auto_interval_minutes: Option<u32>,
    propagate_deletions: bool,
) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    ensure_activation_allowed(&state, &config_dir, &source, &destination, auto_interval_minutes).await?;
    update_job_in(&config_dir, &old_name, &name, &source, &destination, auto_interval_minutes, propagate_deletions)
}

#[tauri::command]
pub fn delete_job(app: AppHandle, name: String) -> Result<(), String> {
    delete_job_in(&app_config_dir(&app)?, &name)
}

#[tauri::command]
pub async fn run_job(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<(), String> {
    run_job_by_name(&app_config_dir(&app)?, &state, &name).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;

    #[test]
    fn push_history_inserts_most_recent_first_and_truncates_at_the_limit() {
        let mut history: Vec<u32> = Vec::new();
        for i in 0..25 {
            push_history(&mut history, i);
        }
        assert_eq!(history.len(), HISTORY_LIMIT, "non deve crescere oltre il limite");
        assert_eq!(history[0], 24, "la voce più recente deve stare in testa");
        assert_eq!(history[HISTORY_LIMIT - 1], 5, "le voci più vecchie escono per prime");
    }

    #[test]
    fn load_from_missing_file_returns_empty() {
        let dir = TempDir::new("jobs-missing");
        assert_eq!(load_from_dir(&dir.path).unwrap(), Vec::new());
    }

    #[test]
    fn create_then_list_roundtrips() {
        let dir = TempDir::new("jobs-roundtrip");
        create_job_in(&dir.path, "backup-foto", "/home/utente/foto", "cubbit:foto", None, false).unwrap();

        let jobs = load_from_dir(&dir.path).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "backup-foto");
        assert_eq!(jobs[0].source, "/home/utente/foto");
        assert_eq!(jobs[0].destination, "cubbit:foto");
        assert!(jobs[0].history.is_empty());
    }

    #[test]
    fn create_job_rejects_duplicate_name() {
        let dir = TempDir::new("jobs-duplicate");
        create_job_in(&dir.path, "dup", "/a", "/b", None, false).unwrap();
        let result = create_job_in(&dir.path, "dup", "/c", "/d", None, false);
        assert!(result.is_err());
        assert_eq!(load_from_dir(&dir.path).unwrap().len(), 1, "il secondo job non deve essere stato salvato");
    }

    #[test]
    fn create_job_rejects_a_second_backup_for_the_same_remote() {
        let dir = TempDir::new("jobs-same-remote");
        create_job_in(&dir.path, "uno", "/locale-a", "cubbit:x", None, false).unwrap();
        let result = create_job_in(&dir.path, "due", "/locale-b", "cubbit:y", None, false);
        assert!(result.is_err());
        assert_eq!(load_from_dir(&dir.path).unwrap().len(), 1);
    }

    #[test]
    fn create_job_rejects_when_the_remote_is_on_the_source_side_already_used_elsewhere() {
        let dir = TempDir::new("jobs-same-remote-source-side");
        create_job_in(&dir.path, "uno", "cubbit:x", "/locale-a", None, false).unwrap();
        // Stavolta il remote compare come sorgente nel nuovo job, mentre nel
        // primo era destinazione: deve comunque scontrarsi.
        let result = create_job_in(&dir.path, "due", "cubbit:y", "/locale-b", None, false);
        assert!(result.is_err());
    }

    #[test]
    fn create_job_allows_different_remotes() {
        let dir = TempDir::new("jobs-different-remotes");
        create_job_in(&dir.path, "uno", "/locale-a", "cubbit:x", None, false).unwrap();
        assert!(create_job_in(&dir.path, "due", "/locale-b", "wasabi:y", None, false).is_ok());
    }

    #[test]
    fn create_job_allows_purely_local_jobs_unrelated_to_any_remote() {
        let dir = TempDir::new("jobs-local-only");
        create_job_in(&dir.path, "uno", "/locale-a", "/locale-b", None, false).unwrap();
        assert!(create_job_in(&dir.path, "due", "/locale-c", "/locale-d", None, false).is_ok());
    }

    #[test]
    fn update_job_can_keep_pointing_at_its_own_remote() {
        let dir = TempDir::new("jobs-update-same-remote-ok");
        create_job_in(&dir.path, "uno", "/locale-a", "cubbit:x", None, false).unwrap();
        assert!(update_job_in(&dir.path, "uno", "uno", "/locale-a", "cubbit:nuovo-percorso", None, false).is_ok());
    }

    #[test]
    fn update_job_rejects_moving_onto_a_remote_already_used_by_another_backup() {
        let dir = TempDir::new("jobs-update-remote-clash");
        create_job_in(&dir.path, "uno", "/locale-a", "cubbit:x", None, false).unwrap();
        create_job_in(&dir.path, "due", "/locale-b", "wasabi:y", None, false).unwrap();
        let result = update_job_in(&dir.path, "due", "due", "/locale-b", "cubbit:altro", None, false);
        assert!(result.is_err());
    }

    #[test]
    fn update_job_changes_fields_and_can_rename() {
        let dir = TempDir::new("jobs-update");
        create_job_in(&dir.path, "vecchio-nome", "/vecchio-a", "/vecchio-b", None, false).unwrap();

        update_job_in(&dir.path, "vecchio-nome", "nuovo-nome", "/nuovo-a", "/nuovo-b", None, false).unwrap();

        let jobs = load_from_dir(&dir.path).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "nuovo-nome");
        assert_eq!(jobs[0].source, "/nuovo-a");
        assert_eq!(jobs[0].destination, "/nuovo-b");
    }

    #[test]
    fn update_job_fails_for_unknown_name() {
        let dir = TempDir::new("jobs-update-unknown");
        assert!(update_job_in(&dir.path, "non-esiste", "x", "/a", "/b", None, false).is_err());
    }

    #[test]
    fn update_job_rejects_rename_onto_an_existing_name() {
        let dir = TempDir::new("jobs-update-rename-conflict");
        create_job_in(&dir.path, "uno", "/a", "/b", None, false).unwrap();
        create_job_in(&dir.path, "due", "/c", "/d", None, false).unwrap();

        let result = update_job_in(&dir.path, "uno", "due", "/a", "/b", None, false);
        assert!(result.is_err());
        let jobs = load_from_dir(&dir.path).unwrap();
        assert_eq!(jobs.iter().find(|j| j.name == "uno").unwrap().source, "/a", "la voce originale non deve essere stata toccata");
    }

    #[test]
    fn create_job_stores_the_automatic_interval() {
        let dir = TempDir::new("jobs-auto-interval");
        create_job_in(&dir.path, "prova", "/a", "/b", Some(15), false).unwrap();
        assert_eq!(load_from_dir(&dir.path).unwrap()[0].auto_interval_minutes, Some(15));
    }

    #[test]
    fn update_job_can_disable_the_automatic_interval() {
        let dir = TempDir::new("jobs-auto-interval-disable");
        create_job_in(&dir.path, "prova", "/a", "/b", Some(15), false).unwrap();
        update_job_in(&dir.path, "prova", "prova", "/a", "/b", None, false).unwrap();
        assert_eq!(load_from_dir(&dir.path).unwrap()[0].auto_interval_minutes, None);
    }

    #[test]
    fn delete_job_removes_it() {
        let dir = TempDir::new("jobs-delete");
        create_job_in(&dir.path, "da-cancellare", "/a", "/b", None, false).unwrap();
        delete_job_in(&dir.path, "da-cancellare").unwrap();
        assert_eq!(load_from_dir(&dir.path).unwrap(), Vec::new());
    }

    #[test]
    fn delete_job_fails_for_unknown_name() {
        let dir = TempDir::new("jobs-delete-unknown");
        assert!(delete_job_in(&dir.path, "non-esiste").is_err());
    }

    #[tokio::test]
    async fn run_job_copies_a_real_file_between_local_folders() {
        let rcd_dir = TempDir::new("jobs-run-rcd");
        let jobs_dir = TempDir::new("jobs-run-jobs");
        let source_dir = TempDir::new("jobs-run-source");
        let dest_dir = TempDir::new("jobs-run-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        std::fs::write(source_dir.path.join("prova.txt"), "contenuto di prova").unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        create_job_in(
            &jobs_dir.path,
            "copia-prova",
            &source_dir.path.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            None,
            false,
        )
        .unwrap();

        let result = run_job_by_name(&jobs_dir.path, &state, "copia-prova").await;
        assert!(result.is_ok(), "la sync dovrebbe riuscire: {result:?}");
        assert!(dest_dir.path.join("prova.txt").exists(), "il file dovrebbe essere stato copiato nella destinazione");

        let jobs = load_from_dir(&jobs_dir.path).unwrap();
        let last_run = jobs[0].history.first().expect("la cronologia dovrebbe contenere l'esecuzione appena fatta");
        assert!(last_run.success);
        assert_eq!(last_run.message, "");
    }

    #[tokio::test]
    async fn run_job_records_failure_for_a_nonexistent_source() {
        let rcd_dir = TempDir::new("jobs-run-fail-rcd");
        let jobs_dir = TempDir::new("jobs-run-fail-jobs");
        let dest_dir = TempDir::new("jobs-run-fail-dest");

        let state = rcd::build_state(rcd_dir.config_path()).await;
        let nonexistent_source = rcd_dir.path.join("questa-cartella-non-esiste");
        create_job_in(
            &jobs_dir.path,
            "sorgente-mancante",
            &nonexistent_source.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            None,
            false,
        )
        .unwrap();

        let result = run_job_by_name(&jobs_dir.path, &state, "sorgente-mancante").await;
        assert!(result.is_err(), "la sync dovrebbe fallire con una sorgente inesistente");

        let jobs = load_from_dir(&jobs_dir.path).unwrap();
        let last_run = jobs[0].history.first().expect("la cronologia dovrebbe contenere l'esecuzione anche in caso di errore");
        assert!(!last_run.success);
        assert!(!last_run.message.is_empty());
    }

    /// Verifica il motivo stesso per cui esiste `propagate_deletions`: con
    /// il default `false` (`sync/copy`), un file presente SOLO nella
    /// destinazione sopravvive anche quando la sorgente è vuota — non deve
    /// mai poter succedere che una sorgente vuota per errore (es. disco
    /// esterno scollegato) svuoti la destinazione.
    #[tokio::test]
    async fn run_job_without_propagate_deletions_never_deletes_extra_destination_files() {
        let rcd_dir = TempDir::new("jobs-no-propagate-rcd");
        let jobs_dir = TempDir::new("jobs-no-propagate-jobs");
        let source_dir = TempDir::new("jobs-no-propagate-source");
        let dest_dir = TempDir::new("jobs-no-propagate-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        std::fs::write(dest_dir.path.join("file-solo-in-dest.txt"), "dato importante").unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        create_job_in(
            &jobs_dir.path,
            "prova-no-propagate",
            &source_dir.path.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            None,
            false,
        )
        .unwrap();

        let result = run_job_by_name(&jobs_dir.path, &state, "prova-no-propagate").await;
        assert!(result.is_ok(), "la copia dovrebbe riuscire anche con sorgente vuota: {result:?}");
        assert!(
            dest_dir.path.join("file-solo-in-dest.txt").exists(),
            "senza propagate_deletions il file solo nella destinazione non deve mai sparire"
        );
    }

    /// Controparte: con `propagate_deletions: true` (`sync/sync`), un file
    /// solo nella destinazione viene davvero rimosso per rispecchiare la
    /// sorgente — comportamento esplicitamente richiesto dall'utente
    /// tramite il flag, non quello di default.
    #[tokio::test]
    async fn run_job_with_propagate_deletions_mirrors_the_source_including_deletions() {
        let rcd_dir = TempDir::new("jobs-propagate-rcd");
        let jobs_dir = TempDir::new("jobs-propagate-jobs");
        let source_dir = TempDir::new("jobs-propagate-source");
        let dest_dir = TempDir::new("jobs-propagate-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        std::fs::write(dest_dir.path.join("file-da-rimuovere.txt"), "non più nella sorgente").unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        create_job_in(
            &jobs_dir.path,
            "prova-propagate",
            &source_dir.path.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            None,
            true,
        )
        .unwrap();

        let result = run_job_by_name(&jobs_dir.path, &state, "prova-propagate").await;
        assert!(result.is_ok(), "il mirror dovrebbe riuscire: {result:?}");
        assert!(
            !dest_dir.path.join("file-da-rimuovere.txt").exists(),
            "con propagate_deletions il file non più nella sorgente deve essere rimosso dalla destinazione"
        );
    }

    #[tokio::test]
    async fn run_job_keeps_a_bounded_history_with_the_most_recent_first() {
        let rcd_dir = TempDir::new("jobs-history-rcd");
        let jobs_dir = TempDir::new("jobs-history-jobs");
        let source_dir = TempDir::new("jobs-history-source");
        let dest_dir = TempDir::new("jobs-history-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        create_job_in(
            &jobs_dir.path,
            "prova",
            &source_dir.path.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            None,
            false,
        )
        .unwrap();

        for i in 0..3 {
            std::fs::write(source_dir.path.join(format!("file{i}.txt")), "x").unwrap();
            run_job_by_name(&jobs_dir.path, &state, "prova").await.unwrap();
        }

        let jobs = load_from_dir(&jobs_dir.path).unwrap();
        assert_eq!(jobs[0].history.len(), 3, "tre esecuzioni dovrebbero produrre tre voci nello storico");
        assert!(jobs[0].history.iter().all(|h| h.success));
    }

    /// Prova che `execute_sync` non tenga il lock su `RcdState` durante
    /// l'attesa del job — se lo tenesse, la chiamata concorrente a
    /// `rcd::call` qui sotto si bloccherebbe fino al termine della sync
    /// invece di rispondere subito. Stesso schema già usato per verificare
    /// l'attesa OAuth in `oauth_remote.rs`.
    #[tokio::test]
    async fn execute_sync_does_not_hold_the_lock_during_the_wait() {
        let rcd_dir = TempDir::new("jobs-lock-rcd");
        let source_dir = TempDir::new("jobs-lock-source");
        let dest_dir = TempDir::new("jobs-lock-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        std::fs::write(source_dir.path.join("prova.txt"), "contenuto").unwrap();

        let state = std::sync::Arc::new(rcd::build_state(rcd_dir.config_path()).await);
        let sync_state = state.clone();
        let source = source_dir.path.to_string_lossy().to_string();
        let destination = dest_dir.path.to_string_lossy().to_string();
        let sync_task =
            tokio::spawn(async move { execute_sync(&sync_state, &source, &destination, false).await });

        // Chiamata concorrente mentre la sync (potenzialmente) è in corso:
        // deve rispondere rapidamente, non restare bloccata dietro al lock.
        let concurrent = tokio::time::timeout(
            Duration::from_secs(5),
            rcd::call(&state, "core/version", serde_json::json!({})),
        )
        .await;
        assert!(concurrent.is_ok(), "la chiamata concorrente non dovrebbe restare bloccata dal lock");

        let outcome = tokio::time::timeout(Duration::from_secs(10), sync_task).await;
        assert!(outcome.is_ok(), "la sync dovrebbe comunque completarsi");
        assert!(outcome.unwrap().unwrap().is_ok());
    }

    /// `tokio::join!` polla i due futuri sullo stesso task, in ordine: il
    /// primo esegue in modo sincrono fino al suo primo `.await` (dentro
    /// `execute_sync`), inserendosi nel set di `running_jobs` prima di
    /// cedere il controllo — il secondo, pollato subito dopo, trova già il
    /// nome occupato e fallisce immediatamente, senza bisogno di sleep o
    /// altre sincronizzazioni per rendere il test deterministico.
    #[tokio::test]
    async fn run_job_by_name_rejects_a_second_concurrent_call_for_the_same_name() {
        let rcd_dir = TempDir::new("jobs-concurrent-guard-rcd");
        let jobs_dir = TempDir::new("jobs-concurrent-guard-jobs");
        let source_dir = TempDir::new("jobs-concurrent-guard-source");
        let dest_dir = TempDir::new("jobs-concurrent-guard-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        std::fs::write(source_dir.path.join("prova.txt"), "contenuto").unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        create_job_in(
            &jobs_dir.path,
            "job-concorrenza-unico",
            &source_dir.path.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            None,
            false,
        )
        .unwrap();

        let (first, second) = tokio::join!(
            run_job_by_name(&jobs_dir.path, &state, "job-concorrenza-unico"),
            run_job_by_name(&jobs_dir.path, &state, "job-concorrenza-unico"),
        );

        assert!(first.is_ok(), "la prima delle due chiamate dovrebbe riuscire: {first:?}");
        assert!(second.is_err(), "la seconda, concorrente, deve essere rifiutata: {second:?}");
        assert!(second.unwrap_err().contains("già in esecuzione"));
    }
}
