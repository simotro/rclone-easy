use crate::rcd::RcdState;
use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Periodo di quiete richiesto dopo l'ultima scrittura in una cartella
/// osservata prima di avviare la sincronizzazione — abbastanza breve da
/// restare nell'ordine di pochi secondi, abbastanza lungo da assorbire
/// scritture non istantanee (salvataggi atomici, copie di file di
/// dimensione moderata) senza sincronizzare a metà di una scrittura.
const DEBOUNCE: Duration = Duration::from_secs(8);

/// Intervallo del ciclo di riconciliazione: rilegge `jobs.toml`/
/// `bisync.toml` per aggiornare le cartelle osservate (job aggiunti,
/// rimossi o modificati) e controlla se qualche cartella "calda" ha
/// raggiunto il periodo di quiete.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum WatchedJob {
    Backup(String),
    Bisync(String),
}

struct WatcherState {
    watcher: Option<RecommendedWatcher>,
    /// Cartella locale osservata -> job che la referenziano (di norma uno
    /// solo, ma niente vieta a due job di condividere la stessa cartella).
    roots: HashMap<PathBuf, Vec<WatchedJob>>,
    /// Ultimo evento visto per un job, in attesa che passi `DEBOUNCE` prima
    /// di considerarlo pronto a sincronizzare.
    pending: HashMap<WatchedJob, Instant>,
}

fn state() -> &'static Mutex<WatcherState> {
    static STATE: OnceLock<Mutex<WatcherState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(WatcherState { watcher: None, roots: HashMap::new(), pending: HashMap::new() }))
}

/// Un percorso locale (scelto dal selettore di cartelle del frontend) è
/// sempre assoluto; nessun nome di remote rclone può iniziare per `/` —
/// stessa logica duplicata in `jobs.rs`/`bisync.rs`/`mounts.rs`, qui
/// riscritta invece di condivisa per restare indipendente dagli altri
/// moduli.
fn is_local_path(fs: &str) -> bool {
    fs.starts_with('/')
}

/// `true` per un evento che rappresenta davvero una modifica al contenuto
/// (creazione, scrittura, rinomina, cancellazione) — `false` per la sola
/// lettura/apertura di un file o un tocco di metadati senza cambio di
/// contenuto (`ATTRIB`, es. mtime). La distinzione è cruciale: `rclone`
/// legge (apre e chiude) ogni file della cartella locale sia per un
/// backup sia per la scansione di un bisync, e sul backend inotify di
/// Linux quella lettura produce comunque eventi (`OPEN`/`CLOSE_NOWRITE`,
/// mappati da `notify` su `EventKind::Access`). Senza questo filtro, la
/// sincronizzazione stessa marcherebbe la propria cartella come "appena
/// modificata", riavviando il debounce e ripartendo da sola senza fine.
fn changes_content(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(_) => false,
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        _ => true,
    }
}

/// Aggiorna lo stato "in attesa di quiete" per i job la cui cartella
/// contiene il percorso dell'evento — gira sul thread dedicato di
/// `notify`, non nel runtime async.
fn on_event(res: notify::Result<Event>) {
    let Ok(event) = res else { return };
    if event.paths.is_empty() || !changes_content(&event.kind) {
        return;
    }
    let mut guard = state().lock().unwrap();
    let WatcherState { roots, pending, .. } = &mut *guard;
    for event_path in &event.paths {
        for (root, jobs) in roots.iter() {
            if event_path.starts_with(root) {
                for job in jobs {
                    pending.insert(job.clone(), Instant::now());
                }
            }
        }
    }
}

/// Quali cartelle locali andrebbero osservate dati gli elenchi attuali di
/// job — funzione pura, separata dalla lettura dei file di config per
/// poterla testare senza filesystem reale. Un backup `fromRemote` (locale
/// come destinazione) non viene incluso: `rclone` legge da lì per scrivere
/// sul remote, non scrive mai nella cartella locale, quindi osservarla non
/// anticiperebbe nulla.
fn desired_roots(jobs: &[crate::jobs::SyncJob], bisync_jobs: &[crate::bisync::BisyncJob]) -> HashMap<PathBuf, Vec<WatchedJob>> {
    let mut desired: HashMap<PathBuf, Vec<WatchedJob>> = HashMap::new();

    for job in jobs {
        if job.auto_interval_minutes.is_none() || !is_local_path(&job.source) {
            continue;
        }
        desired.entry(PathBuf::from(&job.source)).or_default().push(WatchedJob::Backup(job.name.clone()));
    }

    for job in bisync_jobs {
        if job.auto_interval_minutes.is_none() {
            continue;
        }
        // Esattamente uno tra path1/path2 è locale: la UI forza sempre un
        // lato locale e uno remoto per un job di bisync.
        let local_path = if is_local_path(&job.path1) {
            Some(&job.path1)
        } else if is_local_path(&job.path2) {
            Some(&job.path2)
        } else {
            None
        };
        if let Some(path) = local_path {
            desired.entry(PathBuf::from(path)).or_default().push(WatchedJob::Bisync(job.name.clone()));
        }
    }

    desired
}

/// Un'entry pending è pronta a scattare se è passato almeno `DEBOUNCE` da
/// quando è stata vista l'ultima scrittura — funzione pura, separata dal
/// prelievo/rimozione dallo stato condiviso per poterla testare senza
/// dipendere dal clock reale oltre a costruire `Instant` a mano.
fn is_settled(last_event: Instant, now: Instant, debounce: Duration) -> bool {
    now.duration_since(last_event) >= debounce
}

/// Ricalcola le cartelle da osservare e aggiorna il watcher di
/// conseguenza: aggiunge le nuove, rimuove quelle non più rilevanti. Un
/// fallimento di `watch()` (es. limite di sistema sul numero di inotify
/// watch su un albero enorme) è best-effort — quel job resta comunque
/// coperto dal solo scheduler a intervallo fisso, e il tentativo viene
/// ripetuto al prossimo giro.
fn reconcile(config_dir: &Path) {
    let jobs = crate::jobs::load_from_dir(config_dir).unwrap_or_default();
    let bisync_jobs = crate::bisync::load_from_dir(config_dir).unwrap_or_default();
    let desired = desired_roots(&jobs, &bisync_jobs);

    let mut guard = state().lock().unwrap();
    let WatcherState { watcher, roots, pending } = &mut *guard;
    let Some(watcher) = watcher else { return };

    for old_root in roots.keys() {
        if !desired.contains_key(old_root) {
            let _ = watcher.unwatch(old_root);
        }
    }

    let mut new_roots = HashMap::new();
    for (path, jobs) in desired {
        if roots.contains_key(&path) {
            new_roots.insert(path, jobs);
            continue;
        }
        match watcher.watch(&path, RecursiveMode::Recursive) {
            Ok(()) => {
                new_roots.insert(path, jobs);
            }
            Err(e) => {
                eprintln!(
                    "impossibile osservare '{}', resta coperta solo dallo scheduler a intervallo fisso: {e}",
                    path.display()
                );
            }
        }
    }

    // Un job non più osservato non deve lasciare un'entry "calda" residua:
    // altrimenti lo scheduler continuerebbe a saltarlo scambiando una
    // vecchia attività per una recente.
    let desired_jobs: HashSet<&WatchedJob> = new_roots.values().flatten().collect();
    pending.retain(|job, _| desired_jobs.contains(job));

    *roots = new_roots;
}

/// Innesca una sincronizzazione anticipata per ogni job la cui cartella è
/// rimasta quieta abbastanza a lungo, ognuno come task separato — stesso
/// principio di `scheduler.rs::run_due_jobs`: job indipendenti procedono
/// in parallelo, un fallimento (`Err`, incluso "il job è già in
/// esecuzione" se lo scheduler l'ha già avviato nel frattempo) è
/// silenzioso qui, l'esito resta comunque visibile nella cronologia del
/// job.
fn trigger_due(app: &AppHandle, config_dir: &Path) {
    let due: Vec<WatchedJob> = {
        let mut guard = state().lock().unwrap();
        let now = Instant::now();
        let due: Vec<WatchedJob> =
            guard.pending.iter().filter(|(_, &last)| is_settled(last, now, DEBOUNCE)).map(|(job, _)| job.clone()).collect();
        for job in &due {
            guard.pending.remove(job);
        }
        due
    };

    for job in due {
        let app = app.clone();
        let config_dir = config_dir.to_path_buf();
        tauri::async_runtime::spawn(async move {
            match job {
                WatchedJob::Backup(name) => {
                    let state = app.state::<RcdState>();
                    let _ = crate::jobs::run_job_by_name(&config_dir, &state, &name).await;
                }
                WatchedJob::Bisync(name) => {
                    let state = app.state::<RcdState>();
                    // Senza la password (config protetta), il job fallirebbe
                    // in silenzio come qualunque altra esecuzione automatica
                    // — vedi lo stesso commento in scheduler.rs.
                    let password = crate::rcd::current_config_password(&state).await;
                    let _ = crate::bisync::run_bisync_job_by_name(&config_dir, password.as_deref(), &name).await;
                }
            }
        });
    }
}

/// `true` se il job di backup indicato ha attività locale recente non
/// ancora oltre il periodo di quiete — usata da `scheduler.rs` per
/// evitare di lanciare un giro a intervallo fisso proprio mentre il
/// watcher sta per scattare comunque.
pub(crate) fn is_backup_job_hot(name: &str) -> bool {
    is_hot(&WatchedJob::Backup(name.to_string()))
}

/// Equivalente di `is_backup_job_hot` per i job di bisync.
pub(crate) fn is_bisync_job_hot(name: &str) -> bool {
    is_hot(&WatchedJob::Bisync(name.to_string()))
}

fn is_hot(job: &WatchedJob) -> bool {
    state().lock().unwrap().pending.contains_key(job)
}

/// Marca un job come "caldo" senza passare da un vero evento filesystem —
/// solo per i test di `scheduler.rs`, che verificano che un giro a
/// intervallo fisso venga saltato quando il watcher segnala attività
/// recente.
#[cfg(test)]
pub(crate) fn mark_hot_for_test(is_backup: bool, name: &str) {
    let job = if is_backup { WatchedJob::Backup(name.to_string()) } else { WatchedJob::Bisync(name.to_string()) };
    state().lock().unwrap().pending.insert(job, Instant::now());
}

/// Avvia l'osservazione delle cartelle locali coinvolte in backup/bisync
/// con automazione attiva: un watcher di `notify` (inotify su Linux) più
/// un ciclo asincrono che tiene aggiornato l'insieme di cartelle osservate
/// e innesca una sincronizzazione anticipata quando una di esse resta
/// quieta per `DEBOUNCE`, invece di aspettare il prossimo giro dello
/// scheduler a intervallo fisso. Osserva solo il lato locale: modifiche
/// fatte da altri direttamente sul remote restano affidate interamente
/// allo scheduler esistente, invariato.
pub(crate) fn spawn(app: AppHandle, config_dir: PathBuf) {
    match notify::recommended_watcher(on_event) {
        Ok(watcher) => state().lock().unwrap().watcher = Some(watcher),
        Err(e) => {
            eprintln!("impossibile avviare l'osservazione delle cartelle locali, resta solo lo scheduler a intervallo fisso: {e}");
            return;
        }
    }

    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(RECONCILE_INTERVAL).await;
            reconcile(&config_dir);
            trigger_due(&app, &config_dir);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bisync::BisyncJob;
    use crate::jobs::SyncJob;

    fn backup_job(name: &str, source: &str, auto: Option<u32>) -> SyncJob {
        SyncJob {
            name: name.to_string(),
            source: source.to_string(),
            destination: "remote:backup".to_string(),
            auto_interval_minutes: auto,
            propagate_deletions: false,
            history: Vec::new(),
        }
    }

    fn bisync_job(name: &str, path1: &str, path2: &str, auto: Option<u32>) -> BisyncJob {
        BisyncJob {
            name: name.to_string(),
            path1: path1.to_string(),
            path2: path2.to_string(),
            needs_resync: false,
            auto_interval_minutes: auto,
            history: Vec::new(),
        }
    }

    #[test]
    fn desired_roots_includes_a_backup_job_only_when_local_to_remote_and_automatic() {
        let jobs = vec![
            backup_job("manuale", "/home/prova/manuale", None),
            backup_job("verso-remote", "/home/prova/verso-remote", Some(15)),
            backup_job("da-remote", "remote:sorgente", Some(15)),
        ];
        let desired = desired_roots(&jobs, &[]);
        assert_eq!(desired.len(), 1);
        assert!(desired.contains_key(Path::new("/home/prova/verso-remote")));
    }

    #[test]
    fn desired_roots_watches_whichever_side_of_a_bisync_job_is_local() {
        let jobs = vec![
            bisync_job("locale-poi-remote", "/home/prova/a", "remote:a", Some(15)),
            bisync_job("remote-poi-locale", "remote:b", "/home/prova/b", Some(15)),
            bisync_job("manuale", "/home/prova/c", "remote:c", None),
        ];
        let desired = desired_roots(&[], &jobs);
        assert_eq!(desired.len(), 2);
        assert!(desired.contains_key(Path::new("/home/prova/a")));
        assert!(desired.contains_key(Path::new("/home/prova/b")));
    }

    #[test]
    fn desired_roots_groups_multiple_jobs_sharing_the_same_local_folder() {
        let jobs = vec![backup_job("uno", "/home/prova/condivisa", Some(15))];
        let bisync_jobs = vec![bisync_job("due", "/home/prova/condivisa", "remote:due", Some(15))];
        let desired = desired_roots(&jobs, &bisync_jobs);
        assert_eq!(desired.len(), 1);
        assert_eq!(desired[Path::new("/home/prova/condivisa")].len(), 2);
    }

    #[test]
    fn is_settled_waits_for_the_full_debounce_period() {
        let debounce = Duration::from_secs(8);
        let last_event = Instant::now();
        assert!(!is_settled(last_event, last_event + Duration::from_secs(7), debounce), "non ancora quieta un secondo prima");
        assert!(is_settled(last_event, last_event + debounce, debounce), "quieta esattamente al periodo di debounce");
        assert!(is_settled(last_event, last_event + Duration::from_secs(30), debounce), "quieta ben oltre il debounce");
    }

    #[test]
    fn changes_content_ignores_reads_and_metadata_only_touches() {
        assert!(!changes_content(&EventKind::Access(notify::event::AccessKind::Open(notify::event::AccessMode::Read))));
        assert!(!changes_content(&EventKind::Access(notify::event::AccessKind::Close(notify::event::AccessMode::Read))));
        assert!(!changes_content(&EventKind::Modify(ModifyKind::Metadata(notify::event::MetadataKind::WriteTime))));
        assert!(changes_content(&EventKind::Create(notify::event::CreateKind::File)));
        assert!(changes_content(&EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any))));
        assert!(changes_content(&EventKind::Remove(notify::event::RemoveKind::File)));
    }

    /// Riproduce esattamente il bug corretto in questo modulo: `rclone` che
    /// legge un file della cartella osservata (come fa per caricarlo in un
    /// backup, o per scansionarlo durante un bisync) non deve marcare il
    /// job come "caldo" — altrimenti la sincronizzazione stessa
    /// innescherebbe la successiva, senza fine.
    #[tokio::test]
    async fn reading_a_file_in_a_watched_folder_does_not_mark_its_job_as_hot() {
        let source_dir = crate::rcd::tests::TempDir::new("watcher-fs-read-only");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        let file_path = source_dir.path.join("prova.txt");
        std::fs::write(&file_path, "contenuto di prova").unwrap();

        let mut watcher = notify::recommended_watcher(on_event).expect("il watcher dovrebbe avviarsi");
        watcher.watch(&source_dir.path, RecursiveMode::Recursive).expect("dovrebbe riuscire a osservare una cartella reale");

        let job = WatchedJob::Backup("prova-watcher-lettura".to_string());
        {
            let mut guard = state().lock().unwrap();
            guard.roots.insert(source_dir.path.clone(), vec![job.clone()]);
        }

        // Apre e legge il file per intero, poi lo chiude — lo stesso
        // pattern (open, read, close) che `rclone` esegue per caricare un
        // file in un backup, senza mai scriverci sopra.
        let _ = std::fs::read(&file_path).unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(!is_hot(&job), "una semplice lettura non dovrebbe marcare il job come caldo");

        let mut guard = state().lock().unwrap();
        guard.roots.remove(&source_dir.path);
        guard.pending.remove(&job);
    }

    /// Verifica il pezzo che i test puri sopra non possono coprire: che una
    /// scrittura reale sul filesystem, osservata da un vero watcher di
    /// `notify` (non un evento fabbricato a mano), arrivi effettivamente
    /// fino allo stato condiviso tramite `on_event`. La parte "esegue
    /// davvero il job" è già coperta separatamente dai test di
    /// `jobs::run_job_by_name`/`bisync::run_bisync_job_by_name`, che
    /// `trigger_due` si limita a richiamare.
    #[tokio::test]
    async fn a_real_write_to_a_watched_folder_marks_its_job_as_hot() {
        let source_dir = crate::rcd::tests::TempDir::new("watcher-fs-source");
        std::fs::create_dir_all(&source_dir.path).unwrap();

        let mut watcher = notify::recommended_watcher(on_event).expect("il watcher dovrebbe avviarsi");
        watcher.watch(&source_dir.path, RecursiveMode::Recursive).expect("dovrebbe riuscire a osservare una cartella reale");

        let job = WatchedJob::Backup("prova-watcher-fs".to_string());
        {
            let mut guard = state().lock().unwrap();
            guard.roots.insert(source_dir.path.clone(), vec![job.clone()]);
        }

        std::fs::write(source_dir.path.join("prova.txt"), "contenuto di prova").unwrap();

        // Il thread dedicato di `notify` riceve l'evento in modo asincrono
        // rispetto alla scrittura: un breve margine è necessario prima di
        // poter osservare l'effetto nello stato condiviso.
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(is_hot(&job), "una scrittura reale nella cartella osservata dovrebbe marcare il job come caldo");

        // `state()` è statico e condiviso da tutto il binario di test:
        // ripulisce la propria entry per non interferire con altri test.
        let mut guard = state().lock().unwrap();
        guard.roots.remove(&source_dir.path);
        guard.pending.remove(&job);
    }
}
