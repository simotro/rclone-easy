use crate::rcd::RcdState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tauri::{AppHandle, Manager};

/// Job di sincronizzazione bidirezionale (`rclone bisync`) — un concetto
/// proprio di Rclone Easy, stesso schema di `SyncJob`/`MountConfig`.
/// `needs_resync` parte `true` per un job appena creato (nessuna baseline
/// ancora stabilita) e torna `true` se un'esecuzione fallisce in un modo che
/// richiede esplicitamente un resync (rclone lo segnala chiaramente nel suo
/// stesso messaggio d'errore) — un'esecuzione riuscita lo riporta a `false`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BisyncJob {
    pub name: String,
    pub path1: String,
    pub path2: String,
    #[serde(default)]
    pub needs_resync: bool,
    /// `Some(n)` = eseguito da solo ogni `n` minuti dal ciclo interno
    /// dell'app (`scheduler.rs`), oltre al normale "Esegui ora" manuale.
    #[serde(default)]
    pub auto_interval_minutes: Option<u32>,
    /// Le ultime esecuzioni, più recente in testa — stesso schema di
    /// `jobs::SyncJob::history`, vedi lì per il perché della migrazione da
    /// un singolo `lastRun`.
    #[serde(default)]
    pub history: Vec<BisyncRunResult>,
    /// Ultimo dry-run eseguito (`None` se mai fatto) — persistito, stesso
    /// principio di `jobs::SyncJob::last_dry_run`: separato da `history`
    /// perché un dry-run non è un'esecuzione reale, non deve influenzare
    /// `needs_resync` né contare come operazione riuscita.
    #[serde(default)]
    pub last_dry_run: Option<BisyncDryRunReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BisyncRunResult {
    pub success: bool,
    pub message: String,
    pub when_unix: u64,
    #[serde(default)]
    pub conflict_paths: Vec<String>,
    /// Trascrizione leggibile dell'intero log del run (non solo l'ultima
    /// riga d'errore in `message`), così l'utente può capire da sé cosa è
    /// successo davvero prima di decidere come procedere — es. nel caso
    /// "too many deletes" (vedi `error_suggests_force`) qui compaiono i file
    /// che rclone si accingeva a cancellare. `#[serde(default)]` per la
    /// compatibilità con le voci di history già salvate su disco prima di
    /// questo campo.
    #[serde(default)]
    pub log: String,
    /// `true` se il fallimento è esattamente il blocco di sicurezza "too
    /// many deletes" di rclone bisync, che si sblocca solo con `--force` —
    /// mai applicato automaticamente (vedi `run_bisync_job_forced`), solo
    /// su azione esplicita dell'utente dopo aver visto `log`.
    #[serde(default)]
    pub needs_force: bool,
}

/// `BisyncJob` unito allo stato live "in esecuzione ora" — stesso principio
/// di `jobs::SyncJobEntry`: `is_running` non è mai salvato su disco.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BisyncJobEntry {
    pub name: String,
    pub path1: String,
    pub path2: String,
    pub needs_resync: bool,
    pub auto_interval_minutes: Option<u32>,
    pub history: Vec<BisyncRunResult>,
    pub last_dry_run: Option<BisyncDryRunReport>,
    pub is_running: bool,
    pub is_dry_running: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BisyncFile {
    #[serde(default)]
    jobs: Vec<BisyncJob>,
}

fn bisync_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("bisync.toml")
}

pub(crate) fn load_from_dir(config_dir: &Path) -> Result<Vec<BisyncJob>, String> {
    let path = bisync_file_path(config_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("impossibile leggere '{}': {e}", path.display()))?;
    let file: BisyncFile = toml::from_str(&content).map_err(|e| format!("file dei job bisync non valido: {e}"))?;
    Ok(file.jobs)
}

fn save_to_dir(config_dir: &Path, jobs: &[BisyncJob]) -> Result<(), String> {
    std::fs::create_dir_all(config_dir)
        .map_err(|e| format!("impossibile creare '{}': {e}", config_dir.display()))?;
    let file = BisyncFile { jobs: jobs.to_vec() };
    let content = toml::to_string_pretty(&file).map_err(|e| format!("impossibile serializzare i job bisync: {e}"))?;
    crate::fs_atomic::write_atomically(&bisync_file_path(config_dir), &content)
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

fn job_remote_names(job: &BisyncJob) -> [Option<&str>; 2] {
    [remote_name_of(&job.path1), remote_name_of(&job.path2)]
}

/// Riscrive `path1`/`path2` di ogni job bisync che referenzia `old_name` per
/// puntare a `new_name` — usata solo da `remotes::rename_remote_in`, vedi lì
/// per il perché (nessun `config/rename` in rclone via RC).
pub(crate) fn rename_remote_references_in(config_dir: &Path, old_name: &str, new_name: &str) -> Result<(), String> {
    fn renamed(fs: &str, old_name: &str, new_name: &str) -> Option<String> {
        (remote_name_of(fs) == Some(old_name)).then(|| format!("{new_name}:{}", fs.split_once(':').map(|(_, rest)| rest).unwrap_or("")))
    }

    let mut jobs = load_from_dir(config_dir)?;
    let mut changed = false;
    for j in jobs.iter_mut() {
        if let Some(new_path1) = renamed(&j.path1, old_name, new_name) {
            j.path1 = new_path1;
            changed = true;
        }
        if let Some(new_path2) = renamed(&j.path2, old_name, new_name) {
            j.path2 = new_path2;
            changed = true;
        }
    }
    if changed {
        save_to_dir(config_dir, &jobs)?;
    }
    Ok(())
}

/// Rifiuta una seconda sincronizzazione bidirezionale per un remote già
/// usato da un'altra — al massimo una bisync per remote, stessa
/// semplificazione già applicata a mount e backup. `excluding` è il nome
/// della voce in corso di modifica, da non contare come "già esistente".
fn ensure_remotes_not_already_bisynced(
    jobs: &[BisyncJob],
    path1: &str,
    path2: &str,
    excluding: Option<&str>,
) -> Result<(), String> {
    for candidate in [remote_name_of(path1), remote_name_of(path2)].into_iter().flatten() {
        let clash = jobs
            .iter()
            .filter(|j| excluding != Some(j.name.as_str()))
            .any(|j| job_remote_names(j).contains(&Some(candidate)));
        if clash {
            return Err(format!(
                "esiste già una sincronizzazione bidirezionale per '{candidate}': modificala invece di crearne un'altra"
            ));
        }
    }
    Ok(())
}

/// Rifiuta sempre, senza eccezioni, una bisync con `path1`/`path2` pari alla
/// radice del filesystem locale — stesso principio e stessa decisione di
/// `jobs::ensure_not_filesystem_root` (21/8/2026), vedi lì per il perché
/// (la home directory intera invece è un pattern legittimo, gestito solo
/// con un avviso lato frontend, non bloccato).
fn ensure_not_filesystem_root(path1: &str, path2: &str) -> Result<(), String> {
    for (label, fs) in [("path1", path1), ("path2", path2)] {
        if crate::path_safety::is_filesystem_root(fs) {
            return Err(format!("il percorso scelto per {label} ('{fs}') è la radice del filesystem: scegli una sottocartella più specifica"));
        }
    }
    Ok(())
}

pub(crate) fn create_bisync_job_in(
    config_dir: &Path,
    name: &str,
    path1: &str,
    path2: &str,
    auto_interval_minutes: Option<u32>,
) -> Result<(), String> {
    ensure_not_filesystem_root(path1, path2)?;
    let mut jobs = load_from_dir(config_dir)?;
    if jobs.iter().any(|j| j.name == name) {
        return Err(format!("esiste già un job bisync chiamato '{name}': scegli un altro nome"));
    }
    ensure_remotes_not_already_bisynced(&jobs, path1, path2, None)?;
    jobs.push(BisyncJob {
        name: name.to_string(),
        path1: path1.to_string(),
        path2: path2.to_string(),
        needs_resync: true,
        auto_interval_minutes,
        history: Vec::new(),
        last_dry_run: None,
    });
    save_to_dir(config_dir, &jobs)
}

/// `old_name` individua il job da modificare. Se i percorsi cambiano,
/// `needs_resync` torna `true`: la baseline già stabilita si riferisce ai
/// vecchi percorsi, non ha senso confrontarla con quelli nuovi. Un semplice
/// rinomina (percorsi invariati) non tocca `needs_resync`.
fn update_bisync_job_in(
    config_dir: &Path,
    old_name: &str,
    name: &str,
    path1: &str,
    path2: &str,
    auto_interval_minutes: Option<u32>,
) -> Result<(), String> {
    ensure_not_filesystem_root(path1, path2)?;
    let mut jobs = load_from_dir(config_dir)?;
    if name != old_name && jobs.iter().any(|j| j.name == name) {
        return Err(format!("esiste già un job bisync chiamato '{name}': scegli un altro nome"));
    }
    ensure_remotes_not_already_bisynced(&jobs, path1, path2, Some(old_name))?;
    let job =
        jobs.iter_mut().find(|j| j.name == old_name).ok_or_else(|| format!("nessun job bisync chiamato '{old_name}'"))?;
    let paths_changed = job.path1 != path1 || job.path2 != path2;
    job.name = name.to_string();
    job.path1 = path1.to_string();
    job.path2 = path2.to_string();
    job.auto_interval_minutes = auto_interval_minutes;
    if paths_changed {
        job.needs_resync = true;
    }
    save_to_dir(config_dir, &jobs)
}

pub(crate) fn delete_bisync_job_in(config_dir: &Path, name: &str) -> Result<(), String> {
    let mut jobs = load_from_dir(config_dir)?;
    let before = jobs.len();
    jobs.retain(|j| j.name != name);
    if jobs.len() == before {
        return Err(format!("nessun job bisync chiamato '{name}'"));
    }
    save_to_dir(config_dir, &jobs)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Cartella di lavoro dedicata al job (`--workdir`), sotto la config
/// dell'app: senza specificarla esplicitamente rclone ne calcola una da solo
/// codificando i percorsi completi nel nome file, che supera facilmente il
/// limite di lunghezza del filesystem con percorsi non banali (osservato
/// durante lo sviluppo: "file name too long" con un percorso di test
/// nidificato).
fn workdir_for(config_dir: &Path, name: &str) -> PathBuf {
    config_dir.join("bisync-workdirs").join(name)
}

/// Rimuove i codici colore ANSI che rclone include comunque nel campo `msg`
/// anche con `--use-json-log` (verificato: il JSON è valido, ma il testo del
/// messaggio contiene comunque `[...m` per la colorazione pensata per
/// il terminale) — altrimenti finirebbero mostrati letteralmente nella UI.
fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            output.push(c);
        }
    }
    output
}

/// Estrae i percorsi dei file di conflitto generati in un run, leggendo le
/// righe di log JSON (`--use-json-log`, una per evento). Non esiste un
/// endpoint RC per bisync (a differenza di sync/mount) né un riepilogo
/// strutturato dedicato ai conflitti: l'unica fonte è il testo dei log che
/// rclone stesso produce quando rinomina in modo non distruttivo un file in
/// conflitto (`bisync/resolve.go`, messaggio "Renaming PathN copy").
fn extract_conflict_paths(stderr: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in stderr.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let Some(msg) = value.get("msg").and_then(|m| m.as_str()) else { continue };
        let clean = strip_ansi(msg);
        if let Some(idx) = clean.find("Renaming Path") {
            if let Some(path) = clean[idx..].split(" - ").last() {
                let path = path.trim();
                if !path.is_empty() {
                    paths.push(path.to_string());
                }
            }
        }
    }
    paths
}

/// Messaggio più utile disponibile nei log per spiegare un fallimento,
/// ripulito dai codici ANSI — stesso principio di
/// `rcd::error_message_from_response`: mostrare il messaggio più specifico
/// di rclone invece del solo codice di uscita del processo. Tre livelli,
/// dal più specifico al più generico:
/// 1. l'ultima riga di log JSON di livello "error" (caso più comune: rclone
///    logga chiaramente perché ha abortito);
/// 2. se non c'è nessuna riga "error" (es. bisync si ferma per un motivo
///    segnalato solo a livello "notice"/"info"), l'ultimo messaggio di log
///    qualunque sia il livello — meglio di niente;
/// 3. se l'output non è nemmeno JSON valido (es. rclone fallisce prima di
///    inizializzare il logging strutturato), l'ultima riga grezza non
///    vuota di stderr, così da non mostrare mai il solo "terminato con
///    codice N" quando c'è qualunque testo utile da riportare.
fn extract_error_message(stderr: &str) -> Option<String> {
    let mut last_error = None;
    let mut last_any_msg = None;
    let mut last_raw_line = None;
    for line in stderr.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        last_raw_line = Some(strip_ansi(trimmed));
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else { continue };
        let Some(msg) = value.get("msg").and_then(|m| m.as_str()) else { continue };
        let cleaned = strip_ansi(msg);
        if value.get("level").and_then(|l| l.as_str()) == Some("error") {
            last_error = Some(cleaned.clone());
        }
        last_any_msg = Some(cleaned);
    }
    last_error.or(last_any_msg).or(last_raw_line)
}

/// `true` se il messaggio d'errore indica esplicitamente che serve un nuovo
/// resync prima di poter riprovare (rclone lo segnala chiaramente, es.
/// "Bisync aborted. Must run --resync to recover.") — usato per far ripartire
/// automaticamente `needs_resync` così il prossimo "Esegui ora" include già
/// `--resync` senza che l'utente debba capire da solo il messaggio di rclone.
fn error_requires_resync(message: &str) -> bool {
    message.to_lowercase().contains("resync")
}

/// `true` per il blocco di sicurezza specifico di rclone bisync contro
/// cancellazioni di massa ("too many deletes (>50%, N of M) on PathN ...
/// Run with --force if desired."), non per un errore generico — solo in
/// questo caso ha senso offrire in UI un pulsante "esegui comunque", perché
/// solo qui rclone stesso garantisce che aggiungere `--force` risolve senza
/// cambiare nient'altro nel comportamento del run.
fn error_suggests_force(message: &str) -> bool {
    message.to_lowercase().contains("run with --force")
}

/// Numero massimo di righe di log conservate per ogni run (in `history`,
/// persistito su disco in `bisync.toml`): un run su un albero enorme può
/// produrre un log molto lungo, e con `HISTORY_LIMIT` run salvati per job
/// vogliamo evitare una crescita illimitata del file. Tiene le righe più
/// recenti (la coda), dove si trova quasi sempre il motivo di un fallimento.
const MAX_LOG_LINES: usize = 500;

/// Converte il log grezzo di rclone (una riga JSON per evento, con
/// `--use-json-log`) in una trascrizione leggibile "livello  messaggio",
/// ripulita dai codici ANSI — stesso principio di `extract_error_message`
/// ma per l'intero log, non solo l'ultima riga utile. Righe non JSON (es.
/// un crash prima dell'inizializzazione del logging) sono incluse così come
/// sono, ripulite. Troncata alle ultime `MAX_LOG_LINES` righe se più lunga.
fn readable_log(stderr: &str) -> String {
    let lines: Vec<String> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| match serde_json::from_str::<serde_json::Value>(line) {
            Ok(value) => {
                let level = value.get("level").and_then(|l| l.as_str()).unwrap_or("info");
                let msg = value.get("msg").and_then(|m| m.as_str()).unwrap_or(line);
                format!("{level:>7}  {}", strip_ansi(msg))
            }
            Err(_) => strip_ansi(line),
        })
        .collect();

    if lines.len() <= MAX_LOG_LINES {
        return lines.join("\n");
    }
    let skipped = lines.len() - MAX_LOG_LINES;
    let mut truncated = vec![format!("[log troncato: omesse le prime {skipped} righe]")];
    truncated.extend_from_slice(&lines[skipped..]);
    truncated.join("\n")
}

/// Esegue `rclone bisync` come sottoprocesso diretto (non c'è un endpoint RC
/// per bisync, a differenza di sync/mount — SPEC.md prevedeva questo
/// fallback). `Err` solo se non si riesce proprio ad avviare il processo;
/// un fallimento riportato da rclone stesso (exit code diverso da zero)
/// resta dentro il `BisyncRunResult` con `success: false`, così il
/// chiamante può comunque salvare last_run e decidere se serve un resync.
///
/// `config_password`: a differenza di sync/mount (che passano dal demone
/// `rcd`, già avviato con `RCLONE_CONFIG_PASS` — vedi `rcd::start_rcd_in`),
/// questo è un processo `rclone` a sé, che non eredita nulla dal demone.
/// Senza questa password quando la config è protetta, rclone prova a
/// chiederla interattivamente su stdin, che qui è `Stdio::null()`: fallisce
/// subito con un errore "Failed to read line: EOF" invece di un messaggio
/// sensato, interrompendo silenziosamente la sincronizzazione bidirezionale.
/// Costruisce gli argomenti e lancia `rclone bisync` come sottoprocesso
/// diretto, restituendo stato di uscita + stderr grezzo — usata sia da
/// `execute_bisync` (run vero, interpretato in `BisyncRunResult`) sia da
/// `dry_run_bisync_sync` (`--dry-run`, interpretato in
/// `BisyncDryRunReport`): stessa costruzione degli argomenti e stessa
/// gestione della password di config, interpretazioni diverse a valle.
/// `Err` solo se non si riesce proprio ad avviare il processo; un
/// fallimento riportato da rclone stesso (exit code diverso da zero) è nel
/// primo elemento della tupla, non qui — il chiamante decide cosa farne.
async fn run_bisync_subprocess(
    config_path: &Path,
    workdir: &Path,
    path1: &str,
    path2: &str,
    resync: bool,
    force: bool,
    dry_run: bool,
    config_password: Option<&str>,
) -> Result<(std::process::ExitStatus, String), String> {
    std::fs::create_dir_all(workdir)
        .map_err(|e| format!("impossibile creare la cartella di lavoro '{}': {e}", workdir.display()))?;
    clear_stale_lock(workdir);

    let mut args = vec![
        "bisync".to_string(),
        path1.to_string(),
        path2.to_string(),
        "--config".to_string(),
        config_path.to_string_lossy().to_string(),
        "--workdir".to_string(),
        workdir.to_string_lossy().to_string(),
        "--conflict-resolve".to_string(),
        "none".to_string(),
        "--conflict-loser".to_string(),
        "num".to_string(),
        "--conflict-suffix".to_string(),
        "conflitto".to_string(),
        "--suffix-keep-extension".to_string(),
        "--use-json-log".to_string(),
        // Sincronizza anche la creazione (e cancellazione) delle cartelle
        // vuote tra i due lati — senza questo, `bisync` le ignora del
        // tutto, anche se esistono davvero su uno dei due lati.
        "--create-empty-src-dirs".to_string(),
        // Uno "shortcut" di Google Drive che punta a un file ormai
        // cancellato/inaccessibile fa fallire l'intera bisync con un errore
        // critico ("failed to open source object: can't read dangling
        // shortcut") appena prova a copiarlo per davvero — il dry-run
        // invece lo segnala solo come notice, senza tentare la lettura,
        // quindi non lo intercetta. Questo flag (verificato: no-op innocuo
        // per backend non-Drive) fa saltare quello shortcut specifico come
        // se non esistesse, invece di abortire tutto il giro per un singolo
        // collegamento rotto che l'utente non controlla direttamente.
        "--drive-skip-dangling-shortcuts".to_string(),
        // Un solo listing ricorsivo invece di una chiamata per
        // sottocartella — su un caso reale (Google Drive, ~1600
        // sottocartelle) porta un run "a vuoto" da oltre 2 minuti a ~10
        // secondi, verificato empiricamente confrontando i log con e senza
        // questo flag: il tempo andava quasi interamente in pause del
        // pacer tra una chiamata di listing e la successiva, non in un
        // vero rate-limit di Google (mai un errore, solo "Reducing
        // sleep"). rclone lo ignora silenziosamente sui backend che non lo
        // supportano (es. il lato locale), quindi è sicuro passarlo sempre.
        "--fast-list".to_string(),
        // Esclude la cartella dei lock condivisi (vedi remote_lock.rs) dal
        // confronto — altrimenti finirebbe trattata come contenuto vero da
        // sincronizzare tra i due lati, innocuo se `path2` non la contiene
        // affatto.
        "--exclude".to_string(),
        crate::remote_lock::LOCK_FOLDER_EXCLUDE.to_string(),
    ];
    // Invece di cancellare/sovrascrivere per sempre, rclone sposta ciò che
    // verrebbe perso in un "cestino" fratello di `path1`/`path2` (trash.rs)
    // — recuperabile dalla UI. Bisync usa flag dedicati per lato
    // (`--backup-dir1`/`--backup-dir2`, non il generico `--backup-dir`):
    // verificato (21/8/2026) che ciascuno deve stare sullo stesso
    // remote/filesystem del lato che protegge ("Must be a non-overlapping
    // path on the same remote"), e che coesistono senza collisioni con
    // `--conflict-suffix` già usato sopra per i conflitti veri (verificato
    // con un run che innesca entrambi i meccanismi nello stesso giro).
    // Tutto o niente sui due lati, non un OR: `--suffix` non è per-lato in
    // rclone, si applica a entrambi indipendentemente da quali
    // `--backup-dir1/2` sono presenti — con un solo lato protetto (l'altro
    // `None` per il caso limite di `trash::trash_fs_for`, un lato già
    // radice), quel lato riceveva comunque `--suffix` senza un
    // `--backup-dir` a cui appoggiarsi, e finiva rinominato sul posto
    // invece che spostato: file orfani nella cartella live, invisibili al
    // Cestino dell'app (bug reale trovato da Simone il 25/8/2026).
    let backup_dir1 = crate::trash::trash_fs_for(path1);
    let backup_dir2 = crate::trash::trash_fs_for(path2);
    if let (Some(dir1), Some(dir2)) = (backup_dir1, backup_dir2) {
        args.push("--suffix".to_string());
        args.push(crate::trash::trash_suffix(now_unix()));
        args.push("--backup-dir1".to_string());
        args.push(dir1);
        args.push("--backup-dir2".to_string());
        args.push(dir2);
    }
    if resync {
        args.push("--resync".to_string());
        // Senza questo, "--resync" da solo equivale a "--resync-mode path1":
        // per i file già presenti su entrambi i lati ma diversi, vince
        // sempre la cartella locale (path1) indipendentemente da quale
        // versione sia più recente — sorprendente per chi si aspetta che
        // vinca il file modificato più di recente, come infatti ci si
        // aspetterebbe di default.
        args.push("--resync-mode".to_string());
        args.push("newer".to_string());
    }
    if force {
        // Sblocca esplicitamente il blocco di sicurezza "too many deletes"
        // di rclone (vedi `error_suggests_force`) — mai impostato di sua
        // iniziativa dall'app, solo su conferma esplicita dell'utente dopo
        // aver visto il log dettagliato del run fallito.
        args.push("--force".to_string());
    }
    if dry_run {
        args.push("--dry-run".to_string());
    }

    let mut command = tokio::process::Command::new(crate::rclone_bin::resolve_rclone_binary());
    // `Command::output()` (usato prima di questo refactor) configura da sé
    // stdout/stderr come "piped" — passando a `spawn()` +
    // `wait_with_output()` manuale (per poter leggere il PID prima
    // dell'attesa, vedi sotto) va fatto esplicitamente, altrimenti
    // `wait_with_output()` non ha nulla da cui leggere e l'output risulta
    // sempre vuoto.
    command.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::rclone_bin::hide_console_window_tokio(&mut command);
    if let Some(password) = config_password {
        command.env("RCLONE_CONFIG_PASS", password);
    }

    let child = command.spawn().map_err(|e| format!("impossibile avviare 'rclone bisync': {e}"))?;
    // Tracciato solo per poterlo terminare in modo pulito allo spegnimento
    // dell'app (`terminate_running_bisyncs`) — non serve alla mutua
    // esclusione, già garantita altrove da `running_jobs`.
    let pid = child.id();
    if let Some(pid) = pid {
        running_pids().lock().unwrap().insert(pid);
    }
    let output = child.wait_with_output().await;
    if let Some(pid) = pid {
        running_pids().lock().unwrap().remove(&pid);
    }
    let output = output.map_err(|e| format!("impossibile attendere 'rclone bisync': {e}"))?;
    Ok((output.status, String::from_utf8_lossy(&output.stderr).into_owned()))
}

/// Rimuove il lock file lasciato in `workdir` da una precedente esecuzione
/// di bisync il cui processo non esiste più — per qualunque motivo:
/// mancanza di corrente, crash, terminazione forzata del sistema operativo,
/// o anche solo perché questa istanza dell'app non ha fatto in tempo a
/// terminarlo in modo pulito. Senza questo, bisync si rifiuta di procedere
/// ("prior lock file found") indefinitamente finché qualcuno non elimina il
/// lock a mano — capitato realmente, richiedeva intervento manuale ogni
/// volta. Un lock il cui PID esiste ancora NON viene toccato: potrebbe
/// essere un'esecuzione genuinamente sovrapposta (es. scheduler e "Esegui
/// ora" quasi simultanei), che bisync stesso deve continuare a rifiutare.
fn clear_stale_lock(workdir: &Path) {
    let Ok(entries) = std::fs::read_dir(workdir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lck") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let Some(pid) = extract_lock_pid(&content) else { continue };
        if !crate::rclone_bin::process_is_alive(pid) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Estrae il campo `PID` (una stringa nel JSON del lock file di rclone, non
/// un numero) — separata da `clear_stale_lock` per poterla testare con dati
/// sintetici invece che con un vero file su disco.
fn extract_lock_pid(lock_content: &str) -> Option<u32> {
    let value: serde_json::Value = serde_json::from_str(lock_content).ok()?;
    value.get("PID")?.as_str()?.parse().ok()
}

fn running_pids() -> &'static std::sync::Mutex<std::collections::HashSet<u32>> {
    static PIDS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<u32>>> = std::sync::OnceLock::new();
    PIDS.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Termina in modo pulito ogni `rclone bisync` attualmente in esecuzione,
/// tracciato in `running_pids` — chiamata allo spegnimento intenzionale
/// dell'app (`lib.rs::spawn_signal_shutdown_handler`, tray "Esci"), non
/// aiuta contro un crash o una mancanza di corrente (per quello vedi
/// `clear_stale_lock`, che si autoripara al giro successivo qualunque sia
/// la causa). Ricevendo `SIGTERM` rclone stesso rilascia il proprio lock in
/// modo pulito invece di lasciarlo orfano — solo Unix per ora: `kill` non è
/// disponibile su Windows, e lì non esiste ancora un gestore di spegnimento
/// equivalente (vedi `spawn_signal_shutdown_handler`).
#[cfg(unix)]
pub(crate) fn terminate_running_bisyncs() {
    for pid in running_pids().lock().unwrap().iter() {
        let _ = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status();
    }
}

#[cfg(not(unix))]
pub(crate) fn terminate_running_bisyncs() {}

async fn execute_bisync(
    config_path: &Path,
    workdir: &Path,
    path1: &str,
    path2: &str,
    resync: bool,
    force: bool,
    config_password: Option<&str>,
) -> Result<BisyncRunResult, String> {
    let (status, stderr) = run_bisync_subprocess(config_path, workdir, path1, path2, resync, force, false, config_password).await?;

    let conflict_paths = extract_conflict_paths(&stderr);
    let log = readable_log(&stderr);

    let (success, message, needs_force) = if status.success() {
        (true, String::new(), false)
    } else {
        let message =
            extract_error_message(&stderr).unwrap_or_else(|| format!("rclone bisync terminato con codice {:?}", status.code()));
        let needs_force = error_suggests_force(&message);
        (false, message, needs_force)
    };

    Ok(BisyncRunResult { success, message, when_unix: now_unix(), conflict_paths, log, needs_force })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BisyncDryRunReport {
    pub path1_total_files: i64,
    pub path2_total_files: i64,
    pub would_transfer: i64,
    pub would_delete: i64,
    /// Trascrizione leggibile del run di prova — stesso formato di
    /// `BisyncRunResult::log`: i conteggi aggregati sopra dicono "quanti",
    /// il log dice "dove" (bisync è bidirezionale, un singolo numero non
    /// distinguerebbe i trasferimenti verso path1 da quelli verso path2).
    pub log: String,
    pub when_unix: u64,
}

fn extract_file_count(body: &serde_json::Value) -> Result<i64, String> {
    body.get("count").and_then(|v| v.as_i64()).ok_or_else(|| format!("campo 'count' mancante nella risposta di rclone rcd: {body}"))
}

/// Conta i file di un lato con `operations/size` — via RC, indipendente dal
/// sottoprocesso `rclone bisync` (che non ha un demone RC dietro). Usata
/// solo dal dry-run.
async fn count_files(state: &RcdState, fs: &str) -> Result<i64, String> {
    let body = crate::rcd::call(state, "operations/size", serde_json::json!({ "fs": fs })).await?;
    extract_file_count(&body)
}

/// Ultima riga di log JSON con un campo `stats` — rclone la scrive sempre a
/// fine esecuzione, riuscita o no, anche con `--dry-run`: le statistiche
/// riflettono comunque cosa SAREBBE stato trasferito/cancellato (verificato
/// contro un'esecuzione reale: `--dry-run` "salta" l'azione ma la conta lo
/// stesso in `stats.transfers`/`stats.deletes`). Presa da lì invece che dal
/// testo delle righe precedenti ("Queue copy to Path2", ecc.): più robusta,
/// non dipende da frasi che potrebbero cambiare tra versioni di rclone.
fn extract_dry_run_stats(stderr: &str) -> (i64, i64) {
    for line in stderr.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else { continue };
        let Some(stats) = value.get("stats") else { continue };
        let transfers = stats.get("transfers").and_then(|v| v.as_i64()).unwrap_or(0);
        let deletes = stats.get("deletes").and_then(|v| v.as_i64()).unwrap_or(0);
        return (transfers, deletes);
    }
    (0, 0)
}

/// Come `execute_bisync`, ma con `--dry-run`: non sposta né cancella nulla
/// per davvero. Mai passa `force`: non ha senso bypassare la protezione
/// "too many deletes" (`error_suggests_force`) per un'anteprima — se il run
/// vero verrebbe bloccato, il dry-run fallisce con lo stesso identico
/// errore, che è già l'informazione utile (l'utente lo scoprirebbe
/// comunque cliccando "Esegui ora" per davvero).
pub(crate) async fn dry_run_bisync_sync(
    state: &RcdState,
    config_path: &Path,
    workdir: &Path,
    path1: &str,
    path2: &str,
    resync: bool,
    config_password: Option<&str>,
) -> Result<BisyncDryRunReport, String> {
    let path1_total_files = count_files(state, path1).await?;
    let path2_total_files = count_files(state, path2).await?;

    let (status, stderr) = run_bisync_subprocess(config_path, workdir, path1, path2, resync, false, true, config_password).await?;
    if !status.success() {
        let message =
            extract_error_message(&stderr).unwrap_or_else(|| format!("rclone bisync terminato con codice {:?}", status.code()));
        return Err(message);
    }

    let (would_transfer, would_delete) = extract_dry_run_stats(&stderr);
    let log = readable_log(&stderr);

    Ok(BisyncDryRunReport { path1_total_files, path2_total_files, would_transfer, would_delete, log, when_unix: now_unix() })
}

/// Nomi dei job bisync attualmente in esecuzione — stessa cautela di
/// `jobs::running_jobs`: evita che lo scheduler interno rilanci un job il
/// cui giro precedente non è ancora finito, o che un click manuale su
/// "Esegui ora" parta in parallelo a un'esecuzione automatica già in corso
/// (due `rclone bisync` concorrenti sullo stesso workdir sono da evitare).
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

/// Usata da `list_bisync_jobs` per popolare `BisyncJobEntry::is_running`.
fn is_job_running(name: &str) -> bool {
    running_jobs().lock().unwrap().contains(name)
}

/// Sottoinsieme di `running_jobs`, stesso principio di `jobs::dry_running_jobs`
/// — solo per distinguere in UI un dry-run da un run vero
/// (`BisyncJobEntry::is_dry_running`), non partecipa alla mutua esclusione.
fn dry_running_jobs() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static DRY_RUNNING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    DRY_RUNNING.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn is_job_dry_running(name: &str) -> bool {
    dry_running_jobs().lock().unwrap().contains(name)
}

/// `config_password`: vedi il commento su `execute_bisync` — `None` se la
/// config non è protetta. Non passa mai `--force` — vedi
/// `run_bisync_job_by_name_forced` per l'unico punto d'ingresso che può.
/// `state` serve solo per il lock condiviso sul remote (`remote_lock.rs`),
/// non per l'esecuzione vera di bisync (un sottoprocesso diretto, non passa
/// da rcd).
pub(crate) async fn run_bisync_job_by_name(
    config_dir: &Path,
    state: &RcdState,
    config_password: Option<&str>,
    name: &str,
) -> Result<BisyncRunResult, String> {
    run_bisync_job_by_name_with_force(config_dir, state, config_password, name, false).await
}

/// Come `run_bisync_job_by_name`, ma con `--force` — usata solo dal comando
/// `run_bisync_job_forced`, mai dallo scheduler interno: forzare in
/// automatico il blocco di sicurezza "too many deletes" vanificherebbe
/// esattamente la protezione contro cancellazioni di massa che quel blocco
/// offre (vedi il ventesimo slice in memoria per l'analisi rischi disastro
/// originale). Deve restare sempre un'azione manuale ed esplicita.
pub(crate) async fn run_bisync_job_by_name_forced(
    config_dir: &Path,
    state: &RcdState,
    config_password: Option<&str>,
    name: &str,
) -> Result<BisyncRunResult, String> {
    run_bisync_job_by_name_with_force(config_dir, state, config_password, name, true).await
}

async fn run_bisync_job_by_name_with_force(
    config_dir: &Path,
    state: &RcdState,
    config_password: Option<&str>,
    name: &str,
    force: bool,
) -> Result<BisyncRunResult, String> {
    if !running_jobs().lock().unwrap().insert(name.to_string()) {
        return Err(format!("il job '{name}' è già in esecuzione"));
    }

    let result = run_bisync_job_by_name_with_remote_lock(config_dir, state, config_password, name, force).await;

    running_jobs().lock().unwrap().remove(name);
    result
}

/// Acquisisce il lock condiviso sul/sui remote coinvolti (vedi
/// `remote_lock.rs`: più macchine diverse possono avere Rclone Easy
/// configurato sullo stesso remote condiviso, es. un Google Drive di lavoro
/// tenuto disponibile offline su più PC) prima di eseguire per davvero — un
/// conflitto restituisce subito `Err` senza toccare `history`, stesso
/// principio già in vigore un rigo sopra per "il job è già in esecuzione su
/// questa macchina": un'esecuzione automatica (scheduler/watcher, che
/// scartano l'errore con `let _ =`) si limita a saltare questo giro e
/// riprovare al prossimo, un click manuale mostra il messaggio all'utente.
async fn run_bisync_job_by_name_with_remote_lock(
    config_dir: &Path,
    state: &RcdState,
    config_password: Option<&str>,
    name: &str,
    force: bool,
) -> Result<BisyncRunResult, String> {
    let jobs = load_from_dir(config_dir)?;
    let job = jobs.iter().find(|j| j.name == name).ok_or_else(|| format!("nessun job bisync chiamato '{name}'"))?;
    let remotes: Vec<&str> = job_remote_names(job).into_iter().flatten().collect();

    let locks = crate::remote_lock::acquire_all(state, &remotes).await?;
    let result = run_bisync_job_by_name_inner(config_dir, config_password, name, force).await;
    if matches!(&result, Ok(r) if r.success) {
        // Best effort, un lato alla volta: vedi il commento su
        // `trash::cleanup_old_entries_in`, stesso principio già applicato ai
        // backup (jobs.rs).
        crate::trash::cleanup_old_entries_in(state, &job.path1).await;
        crate::trash::cleanup_old_entries_in(state, &job.path2).await;
    }
    crate::remote_lock::release_all(state, locks).await;

    result
}

async fn run_bisync_job_by_name_inner(
    config_dir: &Path,
    config_password: Option<&str>,
    name: &str,
    force: bool,
) -> Result<BisyncRunResult, String> {
    let mut jobs = load_from_dir(config_dir)?;
    let job = jobs.iter().find(|j| j.name == name).cloned().ok_or_else(|| format!("nessun job bisync chiamato '{name}'"))?;

    let workdir = workdir_for(config_dir, name);
    let result = execute_bisync(
        &config_dir.join("rclone.conf"),
        &workdir,
        &job.path1,
        &job.path2,
        job.needs_resync,
        force,
        config_password,
    )
    .await?;

    if let Some(stored) = jobs.iter_mut().find(|j| j.name == name) {
        stored.needs_resync = if result.success { false } else { error_requires_resync(&result.message) };
        crate::jobs::push_history(&mut stored.history, result.clone());
    }
    save_to_dir(config_dir, &jobs)?;

    Ok(result)
}

fn app_config_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))
}

#[tauri::command]
pub fn list_bisync_jobs(app: AppHandle) -> Result<Vec<BisyncJobEntry>, String> {
    let jobs = load_from_dir(&app_config_dir(&app)?)?;
    Ok(jobs
        .into_iter()
        .map(|j| BisyncJobEntry {
            is_running: is_job_running(&j.name),
            is_dry_running: is_job_dry_running(&j.name),
            name: j.name,
            path1: j.path1,
            path2: j.path2,
            needs_resync: j.needs_resync,
            auto_interval_minutes: j.auto_interval_minutes,
            history: j.history,
            last_dry_run: j.last_dry_run,
        })
        .collect())
}

/// Se `auto_interval_minutes` è impostato (la bisync diventa "attiva", vedi
/// `activity.rs`), rifiuta se un altro servizio è già attivo per uno dei
/// remote coinvolti — una bisync solo manuale non ha questa restrizione.
async fn ensure_activation_allowed(
    state: &RcdState,
    config_dir: &Path,
    path1: &str,
    path2: &str,
    auto_interval_minutes: Option<u32>,
) -> Result<(), String> {
    if auto_interval_minutes.is_none() {
        return Ok(());
    }
    for candidate in [remote_name_of(path1), remote_name_of(path2)].into_iter().flatten() {
        crate::activity::ensure_no_other_active_service(state, config_dir, candidate, crate::activity::ServiceKind::Bisync)
            .await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn create_bisync_job(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    name: String,
    path1: String,
    path2: String,
    auto_interval_minutes: Option<u32>,
) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    ensure_activation_allowed(&state, &config_dir, &path1, &path2, auto_interval_minutes).await?;
    create_bisync_job_in(&config_dir, &name, &path1, &path2, auto_interval_minutes)
}

#[tauri::command]
pub async fn update_bisync_job(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    old_name: String,
    name: String,
    path1: String,
    path2: String,
    auto_interval_minutes: Option<u32>,
) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    ensure_activation_allowed(&state, &config_dir, &path1, &path2, auto_interval_minutes).await?;
    update_bisync_job_in(&config_dir, &old_name, &name, &path1, &path2, auto_interval_minutes)
}

#[tauri::command]
pub fn delete_bisync_job(app: AppHandle, name: String) -> Result<(), String> {
    delete_bisync_job_in(&app_config_dir(&app)?, &name)
}

/// Su un thread a parte: chiamare l'API di notifica da dentro un task tokio
/// già in esecuzione (qui siamo dentro un comando async) manda in panic con
/// "Cannot start a runtime from within a runtime" — il plugin apre una
/// propria runtime per il D-Bus di sistema, cosa che Tokio non permette da
/// un suo stesso worker thread. Osservato in pratica: il panic terminava
/// questo task silenziosamente, la notifica non appariva mai. Un thread OS
/// a parte non ha questo vincolo. Stesso schema in
/// `tray.rs::notify_error`/`notify_done`. Condivisa tra `run_bisync_job` e
/// `run_bisync_job_forced`, che devono notificare i conflitti allo stesso
/// modo.
fn notify_conflicts_if_any(app: &AppHandle, name: &str, result: &BisyncRunResult) {
    if !result.success || result.conflict_paths.is_empty() {
        return;
    }
    let app = app.clone();
    let name = name.to_string();
    let count = result.conflict_paths.len();
    std::thread::spawn(move || {
        use tauri_plugin_notification::NotificationExt;
        let _ = app
            .notification()
            .builder()
            .title("Rclone Easy — conflitto rilevato")
            .body(format!(
                "Il job '{name}' ha trovato {count} file modificati su entrambi i lati: nessuna versione è stata persa, entrambe sono state salvate con un suffisso. Apri Rclone Easy per rivederle."
            ))
            .show();
    });
}

/// A differenza dei job di sync (`jobs::run_job`), il risultato non è solo
/// successo/errore: anche un run riuscito può aver generato conflitti da
/// segnalare — per questo restituisce `BisyncRunResult` invece di `()`, e
/// manda una notifica desktop solo in quel caso (un run senza conflitti non
/// ha nulla da far rivedere all'utente).
#[tauri::command]
pub async fn run_bisync_job(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<BisyncRunResult, String> {
    let config_dir = app_config_dir(&app)?;
    let password = crate::rcd::current_config_password(&state).await;
    let result = run_bisync_job_by_name(&config_dir, &state, password.as_deref(), &name).await?;
    notify_conflicts_if_any(&app, &name, &result);
    Ok(result)
}

/// Come `run_bisync_job`, ma passa `--force` a rclone — pensato per l'unico
/// caso in cui serve: sbloccare un run fermato dalla protezione "too many
/// deletes" (`BisyncRunResult::needs_force`), dopo che l'utente ha esaminato
/// `log` in UI e ha scelto esplicitamente di procedere. Il frontend è
/// responsabile di non esporre questa azione se non a fronte di quella
/// condizione precisa (vedi `RemoteRow.svelte`).
#[tauri::command]
pub async fn run_bisync_job_forced(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<BisyncRunResult, String> {
    let config_dir = app_config_dir(&app)?;
    let password = crate::rcd::current_config_password(&state).await;
    let result = run_bisync_job_by_name_forced(&config_dir, &state, password.as_deref(), &name).await?;
    notify_conflicts_if_any(&app, &name, &result);
    Ok(result)
}

/// Anteprima senza salvare niente in `bisync.toml`, ma passa comunque dalla
/// guardia `running_jobs`: non tanto contro un doppio dry-run in sé
/// (innocuo), quanto perché `running_jobs` è anche quello che `tray.rs`
/// interroga per l'icona "in corso" — un dry-run su un remote reale può
/// richiedere secondi/minuti quanto un run vero, senza inserirsi qui non
/// avrebbe alcun riscontro visivo nella tray. Blocca anche un run vero
/// concorrente dello stesso job (e viceversa), per non far girare le due
/// cose insieme sullo stesso workdir.
#[tauri::command]
pub async fn dry_run_bisync_job(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<BisyncDryRunReport, String> {
    let config_dir = app_config_dir(&app)?;
    dry_run_bisync_job_by_name(&config_dir, &state, &name).await
}

/// Come `run_bisync_job_by_name`, ma per un'anteprima: stessa guardia
/// `running_jobs`/`dry_running_jobs` e salva comunque il report in
/// `bisync.toml` (`BisyncJob::last_dry_run`), non solo restituirlo al
/// chiamante. Presa a `config_dir`/`state` invece che ad `AppHandle`:
/// testabile direttamente, senza un contesto Tauri vero.
pub(crate) async fn dry_run_bisync_job_by_name(config_dir: &Path, state: &RcdState, name: &str) -> Result<BisyncDryRunReport, String> {
    if !running_jobs().lock().unwrap().insert(name.to_string()) {
        return Err(format!("il job '{name}' è già in esecuzione"));
    }
    dry_running_jobs().lock().unwrap().insert(name.to_string());

    let result = dry_run_bisync_job_by_name_inner(config_dir, state, name).await;

    dry_running_jobs().lock().unwrap().remove(name);
    running_jobs().lock().unwrap().remove(name);
    result
}

async fn dry_run_bisync_job_by_name_inner(config_dir: &Path, state: &RcdState, name: &str) -> Result<BisyncDryRunReport, String> {
    let jobs = load_from_dir(config_dir)?;
    let job = jobs.iter().find(|j| j.name == name).ok_or_else(|| format!("nessun job bisync chiamato '{name}'"))?;
    let password = crate::rcd::current_config_password(state).await;
    let workdir = workdir_for(config_dir, name);
    let report =
        dry_run_bisync_sync(state, &config_dir.join("rclone.conf"), &workdir, &job.path1, &job.path2, job.needs_resync, password.as_deref())
            .await?;

    let mut jobs = load_from_dir(config_dir)?;
    if let Some(stored) = jobs.iter_mut().find(|j| j.name == name) {
        stored.last_dry_run = Some(report.clone());
        save_to_dir(&config_dir, &jobs)?;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;

    #[test]
    fn extract_lock_pid_reads_the_pid_field_from_the_lock_json() {
        let content = r#"{"Session":"x","PID":"12345","TimeRenewed":"x","TimeExpires":"x"}"#;
        assert_eq!(extract_lock_pid(content), Some(12345));
    }

    #[test]
    fn extract_lock_pid_is_none_for_malformed_or_incomplete_content() {
        assert_eq!(extract_lock_pid("non e' json"), None);
        assert_eq!(extract_lock_pid(r#"{"Session":"x"}"#), None);
    }

    #[test]
    fn clear_stale_lock_removes_a_lock_whose_pid_no_longer_exists() {
        let dir = TempDir::new("bisync-stale-lock");
        std::fs::create_dir_all(&dir.path).unwrap();
        let lock_path = dir.path.join("prova.lck");
        // Ben oltre qualunque pid_max realistico su Linux o Windows.
        std::fs::write(&lock_path, r#"{"PID":"999999999"}"#).unwrap();

        clear_stale_lock(&dir.path);

        assert!(!lock_path.exists(), "un lock orfano (processo morto) va rimosso automaticamente");
    }

    #[test]
    fn clear_stale_lock_leaves_a_lock_whose_pid_is_still_alive() {
        let dir = TempDir::new("bisync-live-lock");
        std::fs::create_dir_all(&dir.path).unwrap();
        let lock_path = dir.path.join("prova.lck");
        std::fs::write(&lock_path, format!(r#"{{"PID":"{}"}}"#, std::process::id())).unwrap();

        clear_stale_lock(&dir.path);

        assert!(lock_path.exists(), "un lock il cui processo esiste ancora non va toccato");
    }

    #[test]
    fn load_from_missing_file_returns_empty() {
        let dir = TempDir::new("bisync-missing");
        assert_eq!(load_from_dir(&dir.path).unwrap(), Vec::new());
    }

    #[test]
    fn create_then_list_roundtrips_and_starts_needing_a_resync() {
        let dir = TempDir::new("bisync-roundtrip");
        create_bisync_job_in(&dir.path, "foto", "/home/utente/foto", "cubbit:foto", None).unwrap();

        let jobs = load_from_dir(&dir.path).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "foto");
        assert_eq!(jobs[0].path1, "/home/utente/foto");
        assert_eq!(jobs[0].path2, "cubbit:foto");
        assert!(jobs[0].needs_resync, "un job appena creato non ha ancora una baseline");
        assert!(jobs[0].history.is_empty());
    }

    #[test]
    fn create_bisync_job_rejects_the_filesystem_root_on_either_side() {
        let dir = TempDir::new("bisync-reject-root");
        assert!(create_bisync_job_in(&dir.path, "prova-1", "/", "cubbit:qualcosa", None).is_err());
        assert!(create_bisync_job_in(&dir.path, "prova-2", "/qualcosa", "/", None).is_err());
        assert_eq!(load_from_dir(&dir.path).unwrap(), Vec::new(), "nessuno dei due job deve essere stato salvato");
    }

    #[test]
    fn create_bisync_job_rejects_duplicate_name() {
        let dir = TempDir::new("bisync-duplicate");
        create_bisync_job_in(&dir.path, "dup", "/a", "/b", None).unwrap();
        let result = create_bisync_job_in(&dir.path, "dup", "/c", "/d", None);
        assert!(result.is_err());
        assert_eq!(load_from_dir(&dir.path).unwrap().len(), 1);
    }

    #[test]
    fn create_bisync_job_rejects_a_second_job_for_the_same_remote() {
        let dir = TempDir::new("bisync-same-remote");
        create_bisync_job_in(&dir.path, "uno", "/locale-a", "cubbit:x", None).unwrap();
        let result = create_bisync_job_in(&dir.path, "due", "/locale-b", "cubbit:y", None);
        assert!(result.is_err());
        assert_eq!(load_from_dir(&dir.path).unwrap().len(), 1);
    }

    #[test]
    fn create_bisync_job_allows_different_remotes() {
        let dir = TempDir::new("bisync-different-remotes");
        create_bisync_job_in(&dir.path, "uno", "/locale-a", "cubbit:x", None).unwrap();
        assert!(create_bisync_job_in(&dir.path, "due", "/locale-b", "wasabi:y", None).is_ok());
    }

    #[test]
    fn update_bisync_job_can_keep_pointing_at_its_own_remote() {
        let dir = TempDir::new("bisync-update-same-remote-ok");
        create_bisync_job_in(&dir.path, "uno", "/locale-a", "cubbit:x", None).unwrap();
        assert!(update_bisync_job_in(&dir.path, "uno", "uno", "/locale-a", "cubbit:nuovo-percorso", None).is_ok());
    }

    #[test]
    fn update_bisync_job_rejects_moving_onto_a_remote_already_used_by_another_job() {
        let dir = TempDir::new("bisync-update-remote-clash");
        create_bisync_job_in(&dir.path, "uno", "/locale-a", "cubbit:x", None).unwrap();
        create_bisync_job_in(&dir.path, "due", "/locale-b", "wasabi:y", None).unwrap();
        let result = update_bisync_job_in(&dir.path, "due", "due", "/locale-b", "cubbit:altro", None);
        assert!(result.is_err());
    }

    #[test]
    fn update_bisync_job_changes_fields_and_can_rename() {
        let dir = TempDir::new("bisync-update");
        create_bisync_job_in(&dir.path, "vecchio-nome", "/vecchio-a", "remote:vecchio-b", None).unwrap();

        update_bisync_job_in(&dir.path, "vecchio-nome", "nuovo-nome", "/nuovo-a", "remote:nuovo-b", None).unwrap();

        let jobs = load_from_dir(&dir.path).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "nuovo-nome");
        assert_eq!(jobs[0].path1, "/nuovo-a");
        assert_eq!(jobs[0].path2, "remote:nuovo-b");
    }

    #[test]
    fn update_bisync_job_fails_for_unknown_name() {
        let dir = TempDir::new("bisync-update-unknown");
        assert!(update_bisync_job_in(&dir.path, "non-esiste", "x", "/a", "/b", None).is_err());
    }

    #[test]
    fn update_bisync_job_rejects_rename_onto_an_existing_name() {
        let dir = TempDir::new("bisync-update-rename-conflict");
        create_bisync_job_in(&dir.path, "uno", "/a", "/b", None).unwrap();
        create_bisync_job_in(&dir.path, "due", "/c", "/d", None).unwrap();

        let result = update_bisync_job_in(&dir.path, "uno", "due", "/a", "/b", None);
        assert!(result.is_err());
        let jobs = load_from_dir(&dir.path).unwrap();
        assert_eq!(jobs.iter().find(|j| j.name == "uno").unwrap().path1, "/a", "la voce originale non deve essere stata toccata");
    }

    #[test]
    fn create_bisync_job_stores_the_automatic_interval() {
        let dir = TempDir::new("bisync-auto-interval");
        create_bisync_job_in(&dir.path, "prova", "/a", "/b", Some(20)).unwrap();
        assert_eq!(load_from_dir(&dir.path).unwrap()[0].auto_interval_minutes, Some(20));
    }

    #[test]
    fn update_bisync_job_can_disable_the_automatic_interval() {
        let dir = TempDir::new("bisync-auto-interval-disable");
        create_bisync_job_in(&dir.path, "prova", "/a", "/b", Some(20)).unwrap();
        update_bisync_job_in(&dir.path, "prova", "prova", "/a", "/b", None).unwrap();
        assert_eq!(load_from_dir(&dir.path).unwrap()[0].auto_interval_minutes, None);
    }

    #[test]
    fn update_bisync_job_resets_needs_resync_when_paths_change_but_not_on_simple_rename() {
        let dir = TempDir::new("bisync-update-resync-flag");
        create_bisync_job_in(&dir.path, "prova", "/a", "/b", None).unwrap();
        // Un job appena creato ha già needs_resync=true di suo: lo azzero a
        // mano per verificare che SOLO il cambio di percorso lo riattivi,
        // non un semplice rinomina.
        {
            let mut jobs = load_from_dir(&dir.path).unwrap();
            jobs[0].needs_resync = false;
            save_to_dir(&dir.path, &jobs).unwrap();
        }

        update_bisync_job_in(&dir.path, "prova", "prova-rinominato", "/a", "/b", None).unwrap();
        assert!(!load_from_dir(&dir.path).unwrap()[0].needs_resync, "un semplice rinomina non deve richiedere un nuovo resync");

        update_bisync_job_in(&dir.path, "prova-rinominato", "prova-rinominato", "/a", "/percorso-nuovo", None).unwrap();
        assert!(load_from_dir(&dir.path).unwrap()[0].needs_resync, "cambiare i percorsi deve richiedere un nuovo resync");
    }

    #[test]
    fn delete_bisync_job_removes_it() {
        let dir = TempDir::new("bisync-delete");
        create_bisync_job_in(&dir.path, "da-cancellare", "/a", "/b", None).unwrap();
        delete_bisync_job_in(&dir.path, "da-cancellare").unwrap();
        assert_eq!(load_from_dir(&dir.path).unwrap(), Vec::new());
    }

    #[test]
    fn delete_bisync_job_fails_for_unknown_name() {
        let dir = TempDir::new("bisync-delete-unknown");
        assert!(delete_bisync_job_in(&dir.path, "non-esiste").is_err());
    }

    #[test]
    fn strip_ansi_removes_color_codes_but_keeps_the_text() {
        let input = "- \u{1b}[36mPath1\u{1b}[0m    \u{1b}[35mRenaming Path1 copy\u{1b}[0m - \u{1b}[36m/tmp/a/doc.conflitto1.txt\u{1b}[0m";
        assert_eq!(strip_ansi(input), "- Path1    Renaming Path1 copy - /tmp/a/doc.conflitto1.txt");
    }

    /// Costruisce una riga di log JSON come la scrive davvero `rclone
    /// --use-json-log`: i codici colore ANSI dentro `msg` sono codificati
    /// come sequenza di escape JSON valida (``, 6 caratteri ASCII), non
    /// come byte ESC grezzo — un byte di controllo non escaped renderebbe
    /// la riga JSON non valida e la farebbe scartare in silenzio da
    /// `serde_json::from_str`.
    fn json_log_line(level: &str, msg_with_esc_markers: &str) -> String {
        let esc = "\\u001b";
        let msg = msg_with_esc_markers.replace("<ESC>", esc);
        format!(r#"{{"level":"{level}","msg":"{msg}"}}"#)
    }

    #[test]
    fn extract_conflict_paths_reads_both_renamed_copies() {
        let stderr = [
            json_log_line(
                "notice",
                "- <ESC>[36mPath1<ESC>[0m    <ESC>[35mRenaming Path1 copy<ESC>[0m                - <ESC>[36m/tmp/a/doc.conflitto1.txt<ESC>[0m",
            ),
            json_log_line(
                "notice",
                "- <ESC>[34mPath2<ESC>[0m    <ESC>[35mRenaming Path2 copy<ESC>[0m                - <ESC>[36m/tmp/b/doc.conflitto2.txt<ESC>[0m",
            ),
            json_log_line("notice", "something unrelated"),
        ]
        .join("\n");

        let paths = extract_conflict_paths(&stderr);
        assert_eq!(paths, vec!["/tmp/a/doc.conflitto1.txt", "/tmp/b/doc.conflitto2.txt"]);
    }

    #[test]
    fn extract_conflict_paths_is_empty_when_there_are_none() {
        let stderr = json_log_line("notice", "Bisync successful");
        assert!(extract_conflict_paths(&stderr).is_empty());
    }

    #[test]
    fn extract_error_message_reads_the_last_error_level_line() {
        let stderr = [
            json_log_line("error", "primo errore"),
            json_log_line("notice", "qualcosa di neutro"),
            json_log_line("error", "<ESC>[31mBisync aborted. Must run --resync to recover.<ESC>[0m"),
        ]
        .join("\n");

        assert_eq!(extract_error_message(&stderr).unwrap(), "Bisync aborted. Must run --resync to recover.");
    }

    #[test]
    fn extract_error_message_falls_back_to_the_last_log_line_when_none_is_level_error() {
        // Un fallimento può fermare bisync senza che rclone logghi una riga
        // "error" esplicita (es. un abort segnalato solo come "notice") —
        // meglio mostrare l'ultimo messaggio disponibile che il solo
        // codice di uscita del processo.
        let stderr = [
            json_log_line("notice", "primo avviso"),
            json_log_line("notice", "Bisync critical error: sync aborted"),
        ]
        .join("\n");

        assert_eq!(extract_error_message(&stderr).unwrap(), "Bisync critical error: sync aborted");
    }

    #[test]
    fn extract_error_message_falls_back_to_the_raw_last_line_when_output_is_not_json() {
        // Se rclone fallisce prima ancora di inizializzare il logging
        // strutturato (es. un errore di parsing degli argomenti), stderr
        // non contiene affatto righe JSON valide.
        let stderr = "Error: unknown flag: --workdirX\n";

        assert_eq!(extract_error_message(stderr).unwrap(), "Error: unknown flag: --workdirX");
    }

    #[test]
    fn extract_error_message_is_none_for_empty_output() {
        assert_eq!(extract_error_message(""), None);
    }

    #[test]
    fn error_requires_resync_matches_the_real_rclone_wording() {
        assert!(error_requires_resync("Bisync aborted. Must run --resync to recover."));
        assert!(!error_requires_resync("rete non raggiungibile"));
    }

    #[test]
    fn error_suggests_force_matches_the_real_rclone_wording() {
        assert!(error_suggests_force(
            "too many deletes (>50%, 21 of 23) on Path1 \"/home/utente/foto\". Run with --force if desired."
        ));
        assert!(!error_suggests_force("rete non raggiungibile"));
        assert!(!error_requires_resync("too many deletes (>50%, 21 of 23) on Path1"), "casi distinti, non si sovrappongono");
    }

    #[test]
    fn readable_log_formats_json_lines_and_strips_ansi() {
        let stderr = [json_log_line("error", "<ESC>[31mtroppi errori<ESC>[0m"), "riga non json".to_string()].join("\n");
        let log = readable_log(&stderr);
        assert_eq!(log, "  error  troppi errori\nriga non json");
    }

    #[test]
    fn readable_log_truncates_to_the_last_lines_when_too_long() {
        let stderr = (0..MAX_LOG_LINES + 10).map(|i| json_log_line("info", &format!("riga {i}"))).collect::<Vec<_>>().join("\n");
        let log = readable_log(&stderr);
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), MAX_LOG_LINES + 1, "un avviso di troncamento in più oltre alle righe tenute");
        assert!(lines[0].contains("troncato"));
        assert!(lines[1].contains("riga 10"), "devono restare le ultime righe, non le prime: {}", lines[1]);
        assert!(lines.last().unwrap().contains(&format!("riga {}", MAX_LOG_LINES + 9)));
    }

    #[tokio::test]
    async fn dry_run_bisync_sync_reports_counts_without_touching_anything() {
        let rcd_dir = TempDir::new("bisync-dryrun-rcd");
        let config_dir = TempDir::new("bisync-dryrun-config");
        let path1_dir = TempDir::new("bisync-dryrun-path1");
        let path2_dir = TempDir::new("bisync-dryrun-path2");
        std::fs::create_dir_all(&config_dir.path).unwrap();
        std::fs::create_dir_all(&path1_dir.path).unwrap();
        std::fs::create_dir_all(&path2_dir.path).unwrap();
        std::fs::write(path1_dir.path.join("baseline.txt"), "presente fin dall'inizio").unwrap();

        let state = crate::rcd::build_state(rcd_dir.config_path()).await;
        let rclone_conf = config_dir.path.join("rclone.conf");
        let workdir = workdir_for(&config_dir.path, "prova");

        // Baseline reale (non dry-run), come farebbe il primo "Esegui ora"
        // dell'utente prima di provare l'anteprima.
        let baseline = execute_bisync(
            &rclone_conf,
            &workdir,
            &path1_dir.path.to_string_lossy(),
            &path2_dir.path.to_string_lossy(),
            true,
            false,
            None,
        )
        .await
        .unwrap();
        assert!(baseline.success, "il resync iniziale dovrebbe riuscire: {baseline:?}");

        std::fs::write(path1_dir.path.join("nuovo.txt"), "solo qui").unwrap();

        let report = dry_run_bisync_sync(
            &state,
            &rclone_conf,
            &workdir,
            &path1_dir.path.to_string_lossy(),
            &path2_dir.path.to_string_lossy(),
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(report.path1_total_files, 2, "baseline.txt + nuovo.txt: {report:?}");
        assert_eq!(report.path2_total_files, 1, "solo baseline.txt è già arrivato: {report:?}");
        assert_eq!(report.would_transfer, 1, "solo nuovo.txt andrebbe trasferito: {report:?}");
        assert_eq!(report.would_delete, 0, "nessuna cancellazione prevista: {report:?}");
        assert!(!report.log.is_empty());

        assert!(!path2_dir.path.join("nuovo.txt").exists(), "il dry-run non deve copiare per davvero");
    }

    #[tokio::test]
    async fn dry_run_bisync_job_by_name_persists_the_report_and_leaves_needs_resync_untouched() {
        let rcd_dir = TempDir::new("bisync-dryrun-persist-rcd");
        let jobs_dir = TempDir::new("bisync-dryrun-persist-jobs");
        let path1_dir = TempDir::new("bisync-dryrun-persist-path1");
        let path2_dir = TempDir::new("bisync-dryrun-persist-path2");
        std::fs::create_dir_all(&jobs_dir.path).unwrap();
        std::fs::create_dir_all(&path1_dir.path).unwrap();
        std::fs::create_dir_all(&path2_dir.path).unwrap();
        std::fs::write(path1_dir.path.join("baseline.txt"), "presente fin dall'inizio").unwrap();

        let state = crate::rcd::build_state(rcd_dir.config_path()).await;
        create_bisync_job_in(
            &jobs_dir.path,
            "prova-dry-persist",
            &path1_dir.path.to_string_lossy(),
            &path2_dir.path.to_string_lossy(),
            None,
        )
        .unwrap();
        assert!(load_from_dir(&jobs_dir.path).unwrap()[0].needs_resync, "un job appena creato non ha ancora una baseline");

        let report = dry_run_bisync_job_by_name(&jobs_dir.path, &state, "prova-dry-persist").await.unwrap();

        let jobs = load_from_dir(&jobs_dir.path).unwrap();
        assert_eq!(jobs[0].last_dry_run, Some(report), "il report deve restare leggibile anche dopo, non solo nella risposta immediata");
        assert!(jobs[0].history.is_empty(), "un dry-run non deve mai comparire nello storico dei run veri");
        assert!(jobs[0].needs_resync, "un dry-run non deve mai stabilire una baseline vera");
        assert!(!path2_dir.path.join("baseline.txt").exists(), "il dry-run non deve copiare per davvero");
    }

    /// Verifica che il primissimo resync preferisca la versione modificata
    /// più di recente, non semplicemente quella locale (path1) — senza
    /// "--resync-mode newer" esplicito, rclone farebbe vincere path1 a
    /// prescindere dalle date, sorprendente per l'utente (vedi il commento
    /// su `execute_bisync`).
    #[tokio::test]
    async fn resync_prefers_the_more_recently_modified_version_over_the_local_side() {
        let jobs_dir = TempDir::new("bisync-resync-newer-jobs");
        let path1_dir = TempDir::new("bisync-resync-newer-path1");
        let path2_dir = TempDir::new("bisync-resync-newer-path2");
        std::fs::create_dir_all(&path1_dir.path).unwrap();
        std::fs::create_dir_all(&path2_dir.path).unwrap();

        // Stesso nome, contenuto diverso, su entrambi i lati PRIMA del primo
        // resync — path2 scritto per ultimo, quindi con mtime più recente.
        std::fs::write(path1_dir.path.join("documento.txt"), "versione vecchia (locale)").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        std::fs::write(path2_dir.path.join("documento.txt"), "versione nuova").unwrap();

        create_bisync_job_in(
            &jobs_dir.path,
            "prova",
            &path1_dir.path.to_string_lossy(),
            &path2_dir.path.to_string_lossy(),
            None,
        )
        .unwrap();
        std::fs::create_dir_all(&jobs_dir.path).unwrap();

        let state = crate::rcd::build_state(jobs_dir.config_path()).await;
        let result = run_bisync_job_by_name(&jobs_dir.path, &state, None, "prova").await.unwrap();
        assert!(result.success, "il resync dovrebbe riuscire: {result:?}");

        let path1_content = std::fs::read_to_string(path1_dir.path.join("documento.txt")).unwrap();
        assert_eq!(
            path1_content, "versione nuova",
            "la versione più recente deve vincere sul resync, non semplicemente quella locale"
        );
    }

    #[tokio::test]
    async fn run_bisync_job_also_syncs_empty_directories_between_both_sides() {
        let jobs_dir = TempDir::new("bisync-empty-dirs-jobs");
        let path1_dir = TempDir::new("bisync-empty-dirs-path1");
        let path2_dir = TempDir::new("bisync-empty-dirs-path2");
        std::fs::create_dir_all(&path1_dir.path).unwrap();
        std::fs::create_dir_all(&path2_dir.path).unwrap();
        // Un placeholder evita la baseline non valida su due cartelle
        // completamente vuote, stesso motivo del test del conflitto sopra.
        std::fs::write(path1_dir.path.join("placeholder.txt"), "presente fin dall'inizio").unwrap();

        create_bisync_job_in(
            &jobs_dir.path,
            "prova-cartelle-vuote",
            &path1_dir.path.to_string_lossy(),
            &path2_dir.path.to_string_lossy(),
            None,
        )
        .unwrap();
        std::fs::create_dir_all(&jobs_dir.path).unwrap();

        let state = crate::rcd::build_state(jobs_dir.config_path()).await;
        let baseline = run_bisync_job_by_name(&jobs_dir.path, &state, None, "prova-cartelle-vuote").await.unwrap();
        assert!(baseline.success, "il resync iniziale dovrebbe riuscire: {baseline:?}");

        std::fs::create_dir_all(path1_dir.path.join("cartella-vuota")).unwrap();

        let result = run_bisync_job_by_name(&jobs_dir.path, &state, None, "prova-cartelle-vuote").await.unwrap();
        assert!(result.success, "{result:?}");
        assert!(
            path2_dir.path.join("cartella-vuota").is_dir(),
            "una cartella vuota creata su un lato dovrebbe comparire anche sull'altro"
        );
    }

    /// Verifica la protezione aggiunta il 21/8/2026 (`--backup-dir1`/
    /// `--backup-dir2`): una cancellazione propagata da un lato all'altro
    /// non deve sparire per sempre, ma finire in un cestino "fratello" del
    /// lato che la riceve (trash.rs), recuperabile — a differenza di un
    /// conflitto vero (entrambi i lati modificati), qui solo `path1` cambia,
    /// quindi bisync non lo segnala affatto come conflitto: senza questa
    /// protezione la versione precedente sparirebbe senza lasciare traccia.
    #[tokio::test]
    async fn run_bisync_job_moves_a_propagated_deletion_to_a_trash_sibling_on_the_receiving_side() {
        let jobs_dir = TempDir::new("bisync-trash-jobs");
        let path1_dir = TempDir::new("bisync-trash-path1");
        let path2_dir = TempDir::new("bisync-trash-path2");
        std::fs::create_dir_all(&path1_dir.path).unwrap();
        std::fs::create_dir_all(&path2_dir.path).unwrap();
        // Tre file: cancellarne uno resta sotto la soglia nativa "too many
        // deletes" (>50%) di bisync, altrimenti il run si fermerebbe da solo
        // prima di arrivare a toccare il cestino, che è proprio quello che
        // vogliamo osservare qui.
        for i in 0..3 {
            std::fs::write(path1_dir.path.join(format!("file{i}.txt")), "x").unwrap();
        }

        create_bisync_job_in(
            &jobs_dir.path,
            "prova-trash",
            &path1_dir.path.to_string_lossy(),
            &path2_dir.path.to_string_lossy(),
            None,
        )
        .unwrap();
        std::fs::create_dir_all(&jobs_dir.path).unwrap();

        let state = crate::rcd::build_state(jobs_dir.config_path()).await;
        let baseline = run_bisync_job_by_name(&jobs_dir.path, &state, None, "prova-trash").await.unwrap();
        assert!(baseline.success, "il resync iniziale dovrebbe riuscire: {baseline:?}");

        std::fs::remove_file(path1_dir.path.join("file0.txt")).unwrap();

        let result = run_bisync_job_by_name(&jobs_dir.path, &state, None, "prova-trash").await.unwrap();
        assert!(result.success, "{result:?}");
        assert!(!path2_dir.path.join("file0.txt").exists(), "la cancellazione deve propagarsi a path2");

        let trash_dir = path2_dir
            .path
            .parent()
            .unwrap()
            .join(format!(".rclone-easy-trash-{}", path2_dir.path.file_name().unwrap().to_string_lossy()));
        let entries: Vec<_> = std::fs::read_dir(&trash_dir)
            .unwrap_or_else(|e| panic!("il cestino dovrebbe esistere in '{}': {e}", trash_dir.display()))
            .collect();
        assert_eq!(entries.len(), 1, "il file cancellato dovrebbe essere finito nel cestino invece di sparire per sempre");
    }

    /// Prova un bisync reale end-to-end: baseline, poi un conflitto genuino
    /// (stesso file modificato su entrambi i lati), verificando che nessuna
    /// delle due versioni vada persa e che compaiano entrambe con suffissi
    /// distinti su entrambi i lati (SPEC: "non sovrascrivere né eliminare
    /// nulla").
    #[tokio::test]
    async fn run_bisync_job_resolves_a_real_conflict_without_losing_either_version() {
        let jobs_dir = TempDir::new("bisync-run-jobs");
        let path1_dir = TempDir::new("bisync-run-path1");
        let path2_dir = TempDir::new("bisync-run-path2");
        std::fs::create_dir_all(&path1_dir.path).unwrap();
        std::fs::create_dir_all(&path2_dir.path).unwrap();
        // Un resync su due cartelle del tutto vuote produce una baseline che
        // rclone stesso considera non valida ("Cannot sync to an empty
        // directory") e richiede un altro resync per riprendersi. Un file
        // già presente evita il caso limite, più vicino comunque a un uso
        // reale (nessuno crea un job bisync su due cartelle che restano
        // vuote per sempre).
        std::fs::write(path1_dir.path.join("placeholder.txt"), "presente fin dall'inizio").unwrap();

        create_bisync_job_in(
            &jobs_dir.path,
            "prova",
            &path1_dir.path.to_string_lossy(),
            &path2_dir.path.to_string_lossy(),
            None,
        )
        .unwrap();
        // execute_bisync usa <config_dir>/rclone.conf: basta che la cartella
        // esista, il file stesso non serve (rclone usa i default se manca).
        std::fs::create_dir_all(&jobs_dir.path).unwrap();

        let state = crate::rcd::build_state(jobs_dir.config_path()).await;
        let baseline = run_bisync_job_by_name(&jobs_dir.path, &state, None, "prova").await.unwrap();
        assert!(baseline.success, "il resync iniziale dovrebbe riuscire: {baseline:?}");
        assert!(!load_from_dir(&jobs_dir.path).unwrap()[0].needs_resync, "dopo un run riuscito non serve più resync");

        std::fs::write(path1_dir.path.join("documento.txt"), "versione lato 1").unwrap();
        std::fs::write(path2_dir.path.join("documento.txt"), "versione lato 2, più lunga delle altre").unwrap();

        let result = run_bisync_job_by_name(&jobs_dir.path, &state, None, "prova").await.unwrap();
        assert!(result.success, "un conflitto risolto da rclone non è un errore per noi: {result:?}");
        assert_eq!(result.conflict_paths.len(), 2, "un conflitto genera due copie rinominate: {result:?}");

        let path1_entries: Vec<_> =
            std::fs::read_dir(&path1_dir.path).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        let path2_entries: Vec<_> =
            std::fs::read_dir(&path2_dir.path).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        assert!(!path1_entries.iter().any(|n| n == "documento.txt"), "il file originale in conflitto non deve restare tale e quale: {path1_entries:?}");
        assert_eq!(path1_entries.len(), 3, "placeholder.txt più le due versioni in conflitto su path1: {path1_entries:?}");
        assert_eq!(path2_entries.len(), 3, "placeholder.txt più le due versioni in conflitto su path2: {path2_entries:?}");

        let all_content: Vec<String> = path1_entries
            .iter()
            .map(|name| std::fs::read_to_string(path1_dir.path.join(name)).unwrap())
            .collect();
        assert!(all_content.iter().any(|c| c == "versione lato 1"), "la versione del lato 1 non deve essere andata persa: {all_content:?}");
        assert!(all_content.iter().any(|c| c.starts_with("versione lato 2")), "la versione del lato 2 non deve essere andata persa: {all_content:?}");
    }

    #[tokio::test]
    async fn run_bisync_job_fails_cleanly_for_an_unknown_job() {
        let dir = TempDir::new("bisync-run-unknown");
        let state = crate::rcd::build_state(dir.config_path()).await;
        let result = run_bisync_job_by_name(&dir.path, &state, None, "non-esiste").await;
        assert!(result.is_err());
    }

    /// Stesso schema deterministico usato in `jobs::run_job_by_name_rejects_
    /// a_second_concurrent_call_for_the_same_name`: `tokio::join!` polla i
    /// due futuri in ordine sullo stesso task, quindi il primo si registra
    /// nel set di `running_jobs` prima di cedere il controllo al primo vero
    /// `.await` (l'avvio del sottoprocesso `rclone bisync`).
    #[tokio::test]
    async fn run_bisync_job_by_name_rejects_a_second_concurrent_call_for_the_same_name() {
        let jobs_dir = TempDir::new("bisync-concurrent-guard-jobs");
        let path1_dir = TempDir::new("bisync-concurrent-guard-path1");
        let path2_dir = TempDir::new("bisync-concurrent-guard-path2");
        std::fs::create_dir_all(&path1_dir.path).unwrap();
        std::fs::create_dir_all(&path2_dir.path).unwrap();

        create_bisync_job_in(
            &jobs_dir.path,
            "job-concorrenza-unico",
            &path1_dir.path.to_string_lossy(),
            &path2_dir.path.to_string_lossy(),
            None,
        )
        .unwrap();

        let state = crate::rcd::build_state(jobs_dir.config_path()).await;
        let (first, second) = tokio::join!(
            run_bisync_job_by_name(&jobs_dir.path, &state, None, "job-concorrenza-unico"),
            run_bisync_job_by_name(&jobs_dir.path, &state, None, "job-concorrenza-unico"),
        );

        assert!(first.is_ok(), "la prima delle due chiamate dovrebbe riuscire: {first:?}");
        assert!(second.is_err(), "la seconda, concorrente, deve essere rifiutata: {second:?}");
        assert!(second.unwrap_err().contains("già in esecuzione"));
    }

    /// Riproduce dal vivo il caso reale segnalato dall'utente: dopo una
    /// baseline stabilita, una cancellazione di massa su un lato (qui: un
    /// disco/cartella locale svuotato per errore) fa scattare la protezione
    /// di rclone contro le cancellazioni di massa ("too many deletes",
    /// soglia di default 50%) — il run normale deve fallire SENZA propagare
    /// le cancellazioni, esporre `needs_force`/`log`, e solo un secondo run
    /// esplicito con `run_bisync_job_by_name_forced` deve completarle
    /// davvero. Verifica quindi sia il blocco sia lo sblocco, non solo il
    /// messaggio d'errore.
    #[tokio::test]
    async fn run_bisync_job_stops_on_too_many_deletes_and_force_unblocks_it() {
        let jobs_dir = TempDir::new("bisync-too-many-deletes-jobs");
        let path1_dir = TempDir::new("bisync-too-many-deletes-path1");
        let path2_dir = TempDir::new("bisync-too-many-deletes-path2");
        std::fs::create_dir_all(&path1_dir.path).unwrap();
        std::fs::create_dir_all(&path2_dir.path).unwrap();
        for i in 0..10 {
            std::fs::write(path1_dir.path.join(format!("file{i}.txt")), format!("contenuto {i}")).unwrap();
        }

        create_bisync_job_in(
            &jobs_dir.path,
            "prova-troppi-cancellati",
            &path1_dir.path.to_string_lossy(),
            &path2_dir.path.to_string_lossy(),
            None,
        )
        .unwrap();
        std::fs::create_dir_all(&jobs_dir.path).unwrap();

        let state = crate::rcd::build_state(jobs_dir.config_path()).await;
        let baseline = run_bisync_job_by_name(&jobs_dir.path, &state, None, "prova-troppi-cancellati").await.unwrap();
        assert!(baseline.success, "il resync iniziale dovrebbe riuscire: {baseline:?}");
        assert_eq!(std::fs::read_dir(&path2_dir.path).unwrap().count(), 10, "la baseline deve aver copiato tutti i file su path2");

        // Cancellazione di massa su path1 (8 file su 10, 80% > soglia 50%).
        for i in 0..8 {
            std::fs::remove_file(path1_dir.path.join(format!("file{i}.txt"))).unwrap();
        }

        let blocked = run_bisync_job_by_name(&jobs_dir.path, &state, None, "prova-troppi-cancellati").await.unwrap();
        assert!(!blocked.success, "una cancellazione di massa deve fermare il run, non propagarlo: {blocked:?}");
        assert!(blocked.needs_force, "questo fallimento specifico deve segnalare che serve --force: {blocked:?}");
        assert!(blocked.message.to_lowercase().contains("too many deletes"), "messaggio inatteso: {}", blocked.message);
        assert!(!blocked.log.is_empty(), "il log dettagliato non deve essere vuoto");
        assert_eq!(
            std::fs::read_dir(&path2_dir.path).unwrap().count(),
            10,
            "path2 non deve aver perso file finché il run è bloccato dalla protezione"
        );

        let forced = run_bisync_job_by_name_forced(&jobs_dir.path, &state, None, "prova-troppi-cancellati").await.unwrap();
        assert!(forced.success, "con --force il run deve completare: {forced:?}");
        assert_eq!(
            std::fs::read_dir(&path2_dir.path).unwrap().count(),
            2,
            "con --force le cancellazioni vanno propagate davvero su path2"
        );
    }
}
