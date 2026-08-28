use crate::rcd::{self, RcdState};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Un singolo file toccato da un run vero (non da un dry-run) — colma
/// l'asimmetria rispetto a bisync, che ha sempre avuto un log dettagliato:
/// prima di questo, `LastRun` diceva solo successo/fallimento aggregato,
/// senza dire QUALI file fossero stati coinvolti. Letto da `core/transferred`
/// (RC), non da un log testuale come bisync: qui il dato arriva già
/// strutturato, niente parsing di righe di log da fare.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TransferEvent {
    pub name: String,
    /// Valore grezzo di rclone: "transferring", "deleting", "moving",
    /// "renaming", "checking"… — tradotto in etichette leggibili solo nel
    /// frontend (i18n), qui tenuto così com'è per non dover tenere un
    /// elenco delle varianti possibili sincronizzato in due posti.
    pub what: String,
    /// Vuoto se il singolo trasferimento è riuscito.
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    pub success: bool,
    pub message: String,
    pub when_unix: u64,
    /// Elenco per-file di quel run — vuoto per le voci scritte da versioni
    /// precedenti dell'app (`#[serde(default)]`), non solo per un run senza
    /// alcun trasferimento reale.
    #[serde(default)]
    pub transfers: Vec<TransferEvent>,
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
    /// Ultimo dry-run eseguito (`None` se mai fatto) — persistito, non solo
    /// tenuto in memoria: l'utente deve poterlo ritrovare anche chiudendo e
    /// riaprendo il modal, o guardando semplicemente la Cronologia, senza
    /// dover rilanciare la prova. Separato da `history` apposta: un dry-run
    /// non è un'esecuzione reale (non ha copiato/cancellato nulla), non deve
    /// contare come "ultima operazione riuscita" né influenzare alcuna
    /// logica pensata per run veri.
    #[serde(default)]
    pub last_dry_run: Option<DryRunReport>,
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
    pub last_dry_run: Option<DryRunReport>,
    pub is_running: bool,
    /// Sottoinsieme di `is_running`: `true` solo se il run in corso è un
    /// dry-run, non uno vero — il frontend lo usa per mostrare "Dry-run in
    /// corso" invece di "Backup in corso" nella riga del remote.
    pub is_dry_running: bool,
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
    crate::fs_atomic::write_atomically(&jobs_file_path(config_dir), &content)
}

/// Nome del remote referenziato da una stringa `fs` (`remoto:percorso`),
/// `None` se è un percorso locale — riconosciuto tramite `Path::is_absolute`
/// (tutti i percorsi locali scelti dal selettore di cartelle sono
/// assoluti), consapevole della piattaforma: un controllo sul solo `/`
/// iniziale classificherebbe erroneamente ogni percorso locale Windows
/// (`C:\Users\...`, che non inizia per `/`) come se fosse un riferimento a
/// un remote, scambiando la lettera di unità per un finto nome di remote.
fn remote_name_of(fs: &str) -> Option<&str> {
    if Path::new(fs).is_absolute() {
        return None;
    }
    fs.split_once(':').map(|(name, _)| name)
}

fn job_remote_names(job: &SyncJob) -> [Option<&str>; 2] {
    [remote_name_of(&job.source), remote_name_of(&job.destination)]
}

/// Riscrive `source`/`destination` di ogni job che referenzia `old_name` per
/// puntare a `new_name` — usata solo da `remotes::rename_remote_in`, vedi lì
/// per il perché (nessun `config/rename` in rclone via RC).
pub(crate) fn rename_remote_references_in(config_dir: &Path, old_name: &str, new_name: &str) -> Result<(), String> {
    fn renamed(fs: &str, old_name: &str, new_name: &str) -> Option<String> {
        (remote_name_of(fs) == Some(old_name)).then(|| format!("{new_name}:{}", fs.split_once(':').map(|(_, rest)| rest).unwrap_or("")))
    }

    let mut jobs = load_from_dir(config_dir)?;
    let mut changed = false;
    for j in jobs.iter_mut() {
        if let Some(new_source) = renamed(&j.source, old_name, new_name) {
            j.source = new_source;
            changed = true;
        }
        if let Some(new_destination) = renamed(&j.destination, old_name, new_name) {
            j.destination = new_destination;
            changed = true;
        }
    }
    if changed {
        save_to_dir(config_dir, &jobs)?;
    }
    Ok(())
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

/// Rifiuta sempre, senza eccezioni, un backup con sorgente o destinazione
/// pari alla radice del filesystem locale (`/`, `C:\`…) — a differenza della
/// home directory intera (pattern di backup legittimo, gestito solo con un
/// avviso di conferma lato frontend, non bloccato qui), non esiste un caso
/// d'uso legittimo per sincronizzare l'intera radice con quest'app: decisione
/// presa con Simone il 21/8/2026. Il frontend (`path_safety::check_dangerous_path`)
/// intercetta già il caso all'atto della scelta della cartella per una UX
/// migliore, ma il blocco vero è qui: l'unico punto che nessuna UI può
/// aggirare.
fn ensure_not_filesystem_root(source: &str, destination: &str) -> Result<(), String> {
    for (label, fs) in [("la sorgente", source), ("la destinazione", destination)] {
        if crate::path_safety::is_filesystem_root(fs) {
            return Err(format!("{label} scelta ('{fs}') è la radice del filesystem: scegli una sottocartella più specifica"));
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
    ensure_not_filesystem_root(source, destination)?;
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
        last_dry_run: None,
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
    ensure_not_filesystem_root(source, destination)?;
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

/// Quanti file della destinazione un run con cancellazioni abilitate può
/// arrivare a cancellare prima che rclone stesso si fermi (`MaxDelete` via
/// RC, equivalente di `--max-delete`) — metà del totale attuale, stessa
/// soglia usata nativamente da bisync ("too many deletes (>50%...)"), per un
/// comportamento coerente tra i due tipi di job. Verificato empiricamente
/// (21/8/2026) che senza `MaxDelete` `sync/sync` cancella il 100% della
/// destinazione senza alcun avviso, a differenza di bisync che si blocca da
/// solo — questa era una lacuna reale, non ipotetica. `"directory not
/// found"` è trattato come destinazione vuota (0 file, quindi nessuna
/// cancellazione possibile comunque) invece di un errore: capita al primo
/// run di un backup verso una destinazione che ancora non esiste, e non deve
/// far fallire il job. Qualunque altro errore invece si propaga: se non
/// possiamo contare la destinazione in modo affidabile non ha senso indovinare
/// una soglia, e la sync stessa incontrerà a breve lo stesso problema.
async fn max_delete_for(info: &crate::rcd::ConnectionInfo, destination: &str) -> Result<i64, String> {
    match count_files(info, destination).await {
        Ok(count) => Ok(((count as f64) * 0.5).ceil() as i64),
        Err(e) if e.contains("directory not found") => Ok(0),
        Err(e) => Err(e),
    }
}

/// Esito di un run vero (non di un dry-run): a differenza di un semplice
/// `Result<(), String>`, un fallimento riportato da rclone stesso (job
/// finito ma con errore) resta qui dentro con `success: false`, non in un
/// `Err` — permette di allegare `transfers` anche a un run fallito (può
/// essere fallito a metà, con alcuni file già toccati prima dell'errore).
/// `Err` da `execute_sync` resta riservato solo a un'impossibilità di
/// eseguire il tentativo stesso (rcd irraggiungibile, risposta malformata),
/// stesso principio già usato da `bisync::execute_bisync`/`BisyncRunResult`.
pub(crate) struct SyncOutcome {
    pub success: bool,
    pub message: String,
    pub transfers: Vec<TransferEvent>,
}

fn extract_transferred(body: &serde_json::Value) -> Vec<TransferEvent> {
    body.get("transferred")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name")?.as_str()?.to_string();
                    let what = item.get("what").and_then(|w| w.as_str()).unwrap_or("").to_string();
                    let error = item.get("error").and_then(|e| e.as_str()).unwrap_or("").to_string();
                    Some(TransferEvent { name, what, error })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Elenco per-file di un job finito, via `core/transferred` (RC) filtrato
/// sul gruppo di quel job (`job/{id}`, stesso raggruppamento già usato per
/// `core/stats` nel dry-run) — a differenza di `core/stats`, che dà solo
/// conteggi aggregati, questo endpoint elenca ogni singolo trasferimento/
/// cancellazione completato (fino agli ultimi 100, limite imposto da rclone
/// stesso, non da questa app). Verificato empiricamente (21/8/2026) contro
/// un demone rcd reale: filtrare per gruppo isola correttamente solo i file
/// di quel job specifico. Best effort: un errore qui (RC irraggiungibile nel
/// brevissimo intervallo tra la fine del job e questa chiamata) non deve far
/// sembrare fallito un run altrimenti riuscito — restituisce semplicemente
/// una lista vuota.
async fn fetch_transfer_events(info: &crate::rcd::ConnectionInfo, job_id: i64) -> Vec<TransferEvent> {
    match info.call("core/transferred", serde_json::json!({ "group": format!("job/{job_id}") })).await {
        Ok(body) => extract_transferred(&body),
        Err(_) => Vec::new(),
    }
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
async fn execute_sync(state: &RcdState, source: &str, destination: &str, propagate_deletions: bool) -> Result<SyncOutcome, String> {
    let info = rcd::connection_info(state).await?;

    let endpoint = if propagate_deletions { "sync/sync" } else { "sync/copy" };
    let mut config = serde_json::json!({
        // Equivalente RC di `--fast-list`: un solo listing ricorsivo
        // invece di una chiamata per sottocartella — su Google Drive
        // (~1600 sottocartelle in un caso reale) porta un run "a
        // vuoto" da oltre 2 minuti a ~10 secondi, verificato
        // empiricamente. rclone lo ignora silenziosamente sui
        // backend che non lo supportano (es. il lato locale), quindi
        // è sicuro passarlo sempre.
        "UseListR": true,
    });
    // Invece di cancellare/sovrascrivere per sempre, rclone sposta ciò che
    // verrebbe perso in un "cestino" fratello della destinazione (trash.rs)
    // — recuperabile dalla UI. Si applica sia a sync/copy (protegge dagli
    // overwrite silenziosi) sia a sync/sync (protegge anche dalle
    // cancellazioni). `None` solo nel caso limite di una destinazione già
    // radice (vedi `trash::trash_fs_for`): il job procede comunque, solo
    // senza questa protezione aggiuntiva. Verificato empiricamente
    // (21/8/2026) contro un demone rcd reale.
    if let Some(trash_fs) = crate::trash::trash_fs_for(destination) {
        config["BackupDir"] = serde_json::json!(trash_fs);
        config["Suffix"] = serde_json::json!(crate::trash::trash_suffix(now_unix()));
        config["SuffixKeepExtension"] = serde_json::json!(true);
    }
    if propagate_deletions {
        config["MaxDelete"] = serde_json::json!(max_delete_for(&info, destination).await?);
    }

    // Le cartelle vuote della sorgente vengono comunque create nella
    // destinazione — senza questo, `sync/copy`/`sync/sync` le ignorano del
    // tutto, anche se esistono davvero in sorgente (comportamento di
    // default di rclone, non specifico di questa app).
    let created = info
        .call(
            endpoint,
            serde_json::json!({
                "srcFs": source,
                "dstFs": destination,
                "createEmptySrcDirs": true,
                "_async": true,
                "_config": config,
                // Esclude la cartella dei lock condivisi (remote_lock.rs)
                // dal confronto — soprattutto per sync/sync (cancellazioni
                // propagate): senza questo, un lock legittimo scritto da
                // un'altra macchina apparirebbe come "file in più" nella
                // destinazione e verrebbe cancellato, vanificando la
                // protezione. Verificato empiricamente che questo filtro
                // esclude davvero il contenuto della cartella. Il cestino
                // (trash.rs) non ha bisogno di un'esclusione analoga: è una
                // cartella fratella della destinazione, non annidata al suo
                // interno, quindi non entra mai nel confronto. Esclude anche
                // un eventuale cestino nativo del sistema lasciato da un
                // vecchio mount FUSE sullo stesso percorso (vedi
                // path_safety::OS_TRASH_EXCLUDE) — stesso rischio di bisync,
                // ricaricare contenuto che l'utente pensava cancellato.
                "_filter": { "ExcludeRule": [crate::remote_lock::LOCK_FOLDER_EXCLUDE, crate::path_safety::OS_TRASH_EXCLUDE] },
            }),
        )
        .await?;
    let job_id = created
        .get("jobid")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("campo 'jobid' mancante nella risposta di rclone rcd: {created}"))?;

    loop {
        let status = info.call("job/status", serde_json::json!({ "jobid": job_id })).await?;
        let finished = status.get("finished").and_then(|v| v.as_bool()).unwrap_or(false);
        if finished {
            let success = status.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
            let message = if success {
                String::new()
            } else {
                status
                    .get("error")
                    .and_then(|v| v.as_str())
                    .filter(|e| !e.is_empty())
                    .unwrap_or("errore sconosciuto durante la sincronizzazione")
                    .to_string()
            };
            let transfers = fetch_transfer_events(&info, job_id).await;
            return Ok(SyncOutcome { success, message, transfers });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DryRunReport {
    pub source_total_files: i64,
    pub destination_total_files: i64,
    pub would_transfer: i64,
    pub would_delete: i64,
    pub when_unix: u64,
}

fn extract_file_count(body: &serde_json::Value) -> Result<i64, String> {
    body.get("count").and_then(|v| v.as_i64()).ok_or_else(|| format!("campo 'count' mancante nella risposta di rclone rcd: {body}"))
}

/// Conta i file di un lato con `operations/size` — molto più leggero di
/// `operations/list` su alberi grandi (rclone soppesa solo un totale invece
/// di elencare ogni voce). Usata solo dal dry-run: un run vero non ha
/// bisogno di questo numero, non vale la pena pagarne il costo lì.
async fn count_files(info: &crate::rcd::ConnectionInfo, fs: &str) -> Result<i64, String> {
    let body = info.call("operations/size", serde_json::json!({ "fs": fs })).await?;
    extract_file_count(&body)
}

/// Come `execute_sync`, ma con `_config: {"DryRun": true}`: rclone confronta
/// i due lati e logga cosa farebbe, senza spostare o cancellare nulla per
/// davvero — le statistiche del job (`core/stats`, filtrate per il suo
/// `group`) dicono quanti trasferimenti/cancellazioni ci sarebbero stati.
/// Una funzione a sé invece di un parametro in più su `execute_sync`: un
/// dry-run non passa mai da `run_job_by_name` (che salva l'esito in
/// `jobs.toml` e partecipa alla guardia anti-sovrapposizione
/// `running_jobs`) — è solo una lettura, non deve lasciare traccia nello
/// storico del job né essere bloccata da/bloccare un run vero in corso.
pub(crate) async fn dry_run_sync(
    state: &RcdState,
    source: &str,
    destination: &str,
    propagate_deletions: bool,
) -> Result<DryRunReport, String> {
    let info = rcd::connection_info(state).await?;

    let source_total_files = count_files(&info, source).await?;
    let destination_total_files = count_files(&info, destination).await?;

    let endpoint = if propagate_deletions { "sync/sync" } else { "sync/copy" };
    let created = info
        .call(
            endpoint,
            serde_json::json!({
                "srcFs": source,
                "dstFs": destination,
                "createEmptySrcDirs": true,
                "_async": true,
                // Vedi il commento gemello in `execute_sync` sul perché
                // `UseListR` — qui affiancato a `DryRun`, non in conflitto.
                "_config": { "DryRun": true, "UseListR": true },
            }),
        )
        .await?;
    let job_id = created
        .get("jobid")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("campo 'jobid' mancante nella risposta di rclone rcd: {created}"))?;

    loop {
        let status = info.call("job/status", serde_json::json!({ "jobid": job_id })).await?;
        let finished = status.get("finished").and_then(|v| v.as_bool()).unwrap_or(false);
        if finished {
            let success = status.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
            if !success {
                let error = status
                    .get("error")
                    .and_then(|v| v.as_str())
                    .filter(|e| !e.is_empty())
                    .unwrap_or("errore sconosciuto durante la prova");
                return Err(error.to_string());
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let stats = info.call("core/stats", serde_json::json!({ "group": format!("job/{job_id}") })).await?;
    let would_transfer = stats.get("transfers").and_then(|v| v.as_i64()).unwrap_or(0);
    let would_delete = stats.get("deletes").and_then(|v| v.as_i64()).unwrap_or(0);

    Ok(DryRunReport { source_total_files, destination_total_files, would_transfer, would_delete, when_unix: now_unix() })
}

/// Stessa guardia `running_jobs` di un run vero (`run_job_by_name`): non
/// tanto per evitare un doppio dry-run in sé (innocuo, di sola lettura),
/// quanto perché `running_jobs` è anche quello che `tray.rs` interroga per
/// decidere se mostrare l'icona "in corso" — senza inserirsi qui, un
/// dry-run su un remote reale (che può richiedere secondi/minuti quanto un
/// run vero) non avrebbe alcun riscontro visivo nella tray, lasciando
/// l'utente a chiedersi se l'app si sia bloccata. Blocca anche un run vero
/// concorrente dello stesso job (e viceversa): evita conteggi confusi se le
/// due cose girassero insieme sullo stesso job.
#[tauri::command]
pub async fn dry_run_job(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<DryRunReport, String> {
    let config_dir = app_config_dir(&app)?;
    dry_run_job_by_name(&config_dir, &state, &name).await
}

/// Come `run_job_by_name`, ma per un'anteprima: stessa guardia
/// `running_jobs`/`dry_running_jobs` e salva comunque il report in
/// `jobs.toml` (`SyncJob::last_dry_run`), non solo restituirlo al
/// chiamante — l'utente deve poterlo ritrovare riaprendo il modal o
/// guardando la Cronologia, non solo nell'istante in cui l'ha lanciato.
/// Presa a `config_dir`/`state` invece che ad `AppHandle`, come
/// `run_job_by_name`: testabile direttamente, senza un contesto Tauri vero.
pub(crate) async fn dry_run_job_by_name(config_dir: &Path, state: &RcdState, name: &str) -> Result<DryRunReport, String> {
    if !running_jobs().lock().unwrap().insert(name.to_string()) {
        return Err(format!("il job '{name}' è già in esecuzione"));
    }
    dry_running_jobs().lock().unwrap().insert(name.to_string());

    let result = dry_run_job_by_name_inner(config_dir, state, name).await;

    dry_running_jobs().lock().unwrap().remove(name);
    running_jobs().lock().unwrap().remove(name);
    result
}

async fn dry_run_job_by_name_inner(config_dir: &Path, state: &RcdState, name: &str) -> Result<DryRunReport, String> {
    let jobs = load_from_dir(config_dir)?;
    let job = jobs.iter().find(|j| j.name == name).ok_or_else(|| format!("nessun job chiamato '{name}'"))?;
    let report = dry_run_sync(state, &job.source, &job.destination, job.propagate_deletions).await?;

    let mut jobs = load_from_dir(config_dir)?;
    if let Some(stored) = jobs.iter_mut().find(|j| j.name == name) {
        stored.last_dry_run = Some(report.clone());
        save_to_dir(config_dir, &jobs)?;
    }

    Ok(report)
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

/// Sottoinsieme di `running_jobs`: quali dei job "in esecuzione" lo sono per
/// un dry-run, non un run vero — solo per distinguerlo in UI
/// (`SyncJobEntry::is_dry_running`), non partecipa a nessuna logica di
/// mutua esclusione (quella resta tutta su `running_jobs`).
fn dry_running_jobs() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static DRY_RUNNING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    DRY_RUNNING.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn is_job_dry_running(name: &str) -> bool {
    dry_running_jobs().lock().unwrap().contains(name)
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

    // Lock condiviso sul/sui remote coinvolti (vedi remote_lock.rs: più
    // macchine diverse possono avere Rclone Easy configurato sullo stesso
    // remote condiviso) — un conflitto restituisce subito `Err` senza
    // toccare `history`, stesso principio di "il job è già in esecuzione su
    // questa macchina" sopra: un'esecuzione automatica (scheduler/watcher,
    // che scartano l'errore con `let _ =`) si limita a saltare questo giro.
    let remotes: Vec<&str> = [remote_name_of(&job.source), remote_name_of(&job.destination)].into_iter().flatten().collect();
    let locks = match crate::remote_lock::acquire_all(state, &remotes).await {
        Ok(locks) => locks,
        Err(e) => {
            running_jobs().lock().unwrap().remove(name);
            return Err(e);
        }
    };

    let result = execute_sync(state, &job.source, &job.destination, job.propagate_deletions).await;
    if matches!(&result, Ok(outcome) if outcome.success) {
        // Best effort: pulisce le voci del cestino più vecchie della
        // finestra di conservazione (trash.rs). Non blocca né fa fallire il
        // job se qualcosa va storto — vedi il commento su
        // `cleanup_old_entries_in`.
        crate::trash::cleanup_old_entries_in(state, &job.destination).await;
    }
    crate::remote_lock::release_all(state, locks).await;

    let last_run = match &result {
        Ok(outcome) => {
            LastRun { success: outcome.success, message: outcome.message.clone(), when_unix: now_unix(), transfers: outcome.transfers.clone() }
        }
        Err(e) => LastRun { success: false, message: e.clone(), when_unix: now_unix(), transfers: Vec::new() },
    };
    if let Some(stored) = jobs.iter_mut().find(|j| j.name == name) {
        push_history(&mut stored.history, last_run);
    }
    let save_result = save_to_dir(config_dir, &jobs);

    running_jobs().lock().unwrap().remove(name);

    save_result?;
    // Riconverte in `Result<(), String>`: `execute_sync` non usa più `Err`
    // per un job che ha semplicemente fallito (vedi il commento su
    // `SyncOutcome`), ma il contratto pubblico di questa funzione resta lo
    // stesso di sempre.
    match result {
        Ok(outcome) if outcome.success => Ok(()),
        Ok(outcome) => Err(outcome.message),
        Err(e) => Err(e),
    }
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
            is_dry_running: is_job_dry_running(&j.name),
            name: j.name,
            source: j.source,
            destination: j.destination,
            auto_interval_minutes: j.auto_interval_minutes,
            propagate_deletions: j.propagate_deletions,
            history: j.history,
            last_dry_run: j.last_dry_run,
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
    fn extract_transferred_reads_name_what_and_error_from_each_entry() {
        let body = serde_json::json!({
            "transferred": [
                { "name": "foto.jpg", "what": "transferring", "error": "" },
                { "name": "vecchio.txt", "what": "deleting", "error": "" },
                { "name": "rotto.bin", "what": "transferring", "error": "connessione interrotta" }
            ]
        });
        let events = extract_transferred(&body);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], TransferEvent { name: "foto.jpg".to_string(), what: "transferring".to_string(), error: String::new() });
        assert_eq!(events[1].what, "deleting");
        assert_eq!(events[2].error, "connessione interrotta");
    }

    #[test]
    fn extract_transferred_is_empty_when_the_field_is_missing() {
        assert_eq!(extract_transferred(&serde_json::json!({})), Vec::new());
    }

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
    fn create_job_rejects_the_filesystem_root_as_source_or_destination() {
        let dir = TempDir::new("jobs-reject-root");
        assert!(create_job_in(&dir.path, "prova-src", "/", "/qualcosa", None, false).is_err());
        assert!(create_job_in(&dir.path, "prova-dst", "/qualcosa", "/", None, false).is_err());
        assert_eq!(load_from_dir(&dir.path).unwrap(), Vec::new(), "nessuno dei due job deve essere stato salvato");
    }

    #[test]
    fn update_job_rejects_moving_onto_the_filesystem_root() {
        let dir = TempDir::new("jobs-update-reject-root");
        create_job_in(&dir.path, "prova", "/a", "/b", None, false).unwrap();
        let result = update_job_in(&dir.path, "prova", "prova", "/", "/b", None, false);
        assert!(result.is_err());
        assert_eq!(load_from_dir(&dir.path).unwrap()[0].source, "/a", "la voce originale non deve essere stata toccata");
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

    /// Verifica la protezione aggiunta il 21/8/2026: la cronologia del
    /// backup non deve più dire solo "successo/fallimento", ma anche QUALI
    /// file sono stati coinvolti — colma l'asimmetria rispetto a bisync, che
    /// aveva già un log dettagliato mentre il backup aveva solo un esito
    /// aggregato.
    #[tokio::test]
    async fn run_job_records_which_files_were_transferred() {
        let rcd_dir = TempDir::new("jobs-transfers-rcd");
        let jobs_dir = TempDir::new("jobs-transfers-jobs");
        let source_dir = TempDir::new("jobs-transfers-source");
        let dest_dir = TempDir::new("jobs-transfers-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        std::fs::write(source_dir.path.join("nuovo.txt"), "contenuto").unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        create_job_in(
            &jobs_dir.path,
            "prova-transfers",
            &source_dir.path.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            None,
            false,
        )
        .unwrap();

        let result = run_job_by_name(&jobs_dir.path, &state, "prova-transfers").await;
        assert!(result.is_ok(), "la sync dovrebbe riuscire: {result:?}");

        let jobs = load_from_dir(&jobs_dir.path).unwrap();
        let last_run = jobs[0].history.first().unwrap();
        assert_eq!(last_run.transfers.len(), 1, "dovrebbe esserci esattamente un file trasferito: {:?}", last_run.transfers);
        assert_eq!(last_run.transfers[0].name, "nuovo.txt");
        assert_eq!(last_run.transfers[0].what, "transferring");
        assert_eq!(last_run.transfers[0].error, "");
    }

    #[tokio::test]
    async fn run_job_also_creates_empty_source_directories_on_the_destination() {
        let rcd_dir = TempDir::new("jobs-run-empty-rcd");
        let jobs_dir = TempDir::new("jobs-run-empty-jobs");
        let source_dir = TempDir::new("jobs-run-empty-source");
        let dest_dir = TempDir::new("jobs-run-empty-dest");
        std::fs::create_dir_all(source_dir.path.join("cartella-vuota")).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        create_job_in(
            &jobs_dir.path,
            "copia-cartella-vuota",
            &source_dir.path.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            None,
            false,
        )
        .unwrap();

        let result = run_job_by_name(&jobs_dir.path, &state, "copia-cartella-vuota").await;
        assert!(result.is_ok(), "la sync dovrebbe riuscire: {result:?}");
        assert!(
            dest_dir.path.join("cartella-vuota").is_dir(),
            "la cartella vuota della sorgente dovrebbe comunque comparire nella destinazione"
        );
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
    async fn dry_run_sync_reports_counts_without_copying_anything() {
        let rcd_dir = TempDir::new("jobs-dryrun-rcd");
        let source_dir = TempDir::new("jobs-dryrun-source");
        let dest_dir = TempDir::new("jobs-dryrun-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        std::fs::write(source_dir.path.join("da-copiare.txt"), "nuovo").unwrap();
        std::fs::write(dest_dir.path.join("gia-presente.txt"), "esistente").unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        let report =
            dry_run_sync(&state, &source_dir.path.to_string_lossy(), &dest_dir.path.to_string_lossy(), false)
                .await
                .unwrap();

        assert_eq!(report.source_total_files, 1);
        assert_eq!(report.destination_total_files, 1);
        assert_eq!(report.would_transfer, 1, "il file mancante in destinazione dovrebbe risultare da trasferire: {report:?}");
        assert_eq!(report.would_delete, 0, "sync/copy non pianifica mai cancellazioni: {report:?}");

        assert!(!dest_dir.path.join("da-copiare.txt").exists(), "il dry-run non deve copiare per davvero");
        assert!(source_dir.path.join("da-copiare.txt").exists(), "il dry-run non deve toccare la sorgente");
        assert!(dest_dir.path.join("gia-presente.txt").exists(), "il dry-run non deve cancellare per davvero");
    }

    #[tokio::test]
    async fn dry_run_sync_with_propagate_deletions_reports_planned_deletions_without_deleting() {
        let rcd_dir = TempDir::new("jobs-dryrun-delete-rcd");
        let source_dir = TempDir::new("jobs-dryrun-delete-source");
        let dest_dir = TempDir::new("jobs-dryrun-delete-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        std::fs::write(dest_dir.path.join("non-piu-in-sorgente.txt"), "da rimuovere secondo il mirror").unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        let report =
            dry_run_sync(&state, &source_dir.path.to_string_lossy(), &dest_dir.path.to_string_lossy(), true)
                .await
                .unwrap();

        assert_eq!(report.would_delete, 1, "con propagate_deletions il file solo in destinazione dovrebbe risultare da cancellare: {report:?}");
        assert!(
            dest_dir.path.join("non-piu-in-sorgente.txt").exists(),
            "il dry-run non deve cancellare per davvero nemmeno con propagate_deletions"
        );
    }

    #[tokio::test]
    async fn dry_run_job_by_name_persists_the_report_and_leaves_no_trace_in_history() {
        let rcd_dir = TempDir::new("jobs-dryrun-persist-rcd");
        let jobs_dir = TempDir::new("jobs-dryrun-persist-jobs");
        let source_dir = TempDir::new("jobs-dryrun-persist-source");
        let dest_dir = TempDir::new("jobs-dryrun-persist-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        std::fs::write(source_dir.path.join("nuovo.txt"), "x").unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        create_job_in(&jobs_dir.path, "prova-dry-persist", &source_dir.path.to_string_lossy(), &dest_dir.path.to_string_lossy(), None, false)
            .unwrap();

        let report = dry_run_job_by_name(&jobs_dir.path, &state, "prova-dry-persist").await.unwrap();
        assert_eq!(report.would_transfer, 1);

        let jobs = load_from_dir(&jobs_dir.path).unwrap();
        assert_eq!(jobs[0].last_dry_run, Some(report), "il report deve restare leggibile anche dopo, non solo nella risposta immediata");
        assert!(jobs[0].history.is_empty(), "un dry-run non deve mai comparire nello storico dei run veri");
        assert!(!is_job_running("prova-dry-persist"), "la guardia va rilasciata a fine dry-run");
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

    /// Verifica la protezione aggiunta il 21/8/2026: con `propagate_deletions`
    /// un file rimosso dalla destinazione non deve sparire per sempre, ma
    /// finire in un cestino "fratello" della destinazione (trash.rs),
    /// recuperabile. Trovato empiricamente che senza questa protezione
    /// `sync/sync` cancellava per sempre senza alcun avviso.
    #[tokio::test]
    async fn run_job_with_propagate_deletions_moves_removed_files_to_a_trash_sibling_instead_of_deleting_them() {
        let rcd_dir = TempDir::new("jobs-trash-rcd");
        let jobs_dir = TempDir::new("jobs-trash-jobs");
        let source_dir = TempDir::new("jobs-trash-source");
        let dest_dir = TempDir::new("jobs-trash-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        std::fs::write(dest_dir.path.join("da-rimuovere.txt"), "contenuto importante").unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        create_job_in(
            &jobs_dir.path,
            "prova-trash",
            &source_dir.path.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            None,
            true,
        )
        .unwrap();

        let result = run_job_by_name(&jobs_dir.path, &state, "prova-trash").await;
        assert!(result.is_ok(), "il mirror dovrebbe riuscire: {result:?}");
        assert!(!dest_dir.path.join("da-rimuovere.txt").exists(), "il file deve sparire dalla destinazione");

        let trash_dir = dest_dir
            .path
            .parent()
            .unwrap()
            .join(format!(".rclone-easy-trash-{}", dest_dir.path.file_name().unwrap().to_string_lossy()));
        let entries: Vec<_> = std::fs::read_dir(&trash_dir)
            .unwrap_or_else(|e| panic!("il cestino dovrebbe esistere in '{}': {e}", trash_dir.display()))
            .collect();
        assert_eq!(entries.len(), 1, "il file cancellato dovrebbe essere finito nel cestino invece di sparire per sempre");
    }

    /// Verifica la protezione aggiunta il 21/8/2026: senza `MaxDelete`,
    /// verificato empiricamente che `sync/sync` cancella il 100% della
    /// destinazione senza fermarsi. Con la sorgente vuota e 10 file in
    /// destinazione, un mirror ne cancellerebbe tutti e 10 (100% > 50%) — il
    /// run deve fermarsi prima, lasciandone qualcuno.
    #[tokio::test]
    async fn run_job_with_propagate_deletions_refuses_to_delete_more_than_half_the_destination() {
        let rcd_dir = TempDir::new("jobs-maxdelete-rcd");
        let jobs_dir = TempDir::new("jobs-maxdelete-jobs");
        let source_dir = TempDir::new("jobs-maxdelete-source");
        let dest_dir = TempDir::new("jobs-maxdelete-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();
        for i in 0..10 {
            std::fs::write(dest_dir.path.join(format!("file{i}.txt")), "x").unwrap();
        }

        let state = rcd::build_state(rcd_dir.config_path()).await;
        create_job_in(
            &jobs_dir.path,
            "prova-maxdelete",
            &source_dir.path.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            None,
            true,
        )
        .unwrap();

        let result = run_job_by_name(&jobs_dir.path, &state, "prova-maxdelete").await;
        assert!(result.is_err(), "il run dovrebbe fermarsi prima di cancellare più della soglia: {result:?}");

        let remaining = std::fs::read_dir(&dest_dir.path).unwrap().count();
        assert!(remaining > 0, "non tutti i file dovrebbero essere stati cancellati: ne restano {remaining}");
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
        let sync_result = outcome.unwrap().unwrap();
        assert!(sync_result.is_ok());
        assert!(sync_result.unwrap().success);
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
