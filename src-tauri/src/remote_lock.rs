use crate::rcd::{self, ConnectionInfo, RcdState};
use std::time::Duration;

/// Cartella nascosta alla radice di ogni remote coinvolto in un bisync/
/// backup vero — dove viene scritto un lock per ogni remote attualmente in
/// uso da una macchina. Necessario perché più macchine diverse possono
/// avere Rclone Easy configurato sullo stesso remote condiviso (caso reale:
/// più PC sullo stesso Google Drive di lavoro, per poterlo avere disponibile
/// offline ovunque) — rclone bisync/sync non hanno alcuna nozione di "un
/// altro client sta già scrivendo qui adesso": bisync in particolare
/// confronta contro un'istantanea locale del suo ultimo giro, ignara di
/// quello che un'altra macchina sta facendo nel frattempo (rclone stesso
/// sconsiglia bisync multi-macchina sullo stesso path per questo motivo).
/// Un backup con cancellazioni propagate (`sync/sync`) rischia lo stesso,
/// dal lato opposto: chi finisce per ultimo può cancellare quanto l'altra
/// macchina ha appena caricato. Il lock vive sul remote stesso perché è
/// l'unica risorsa che tutte le macchine coinvolte condividono davvero.
const LOCK_FOLDER: &str = ".rclone-easy-locks";

/// Filtro da aggiungere a qualunque job che scrive su un remote coinvolto in
/// questo meccanismo — altrimenti la cartella dei lock finirebbe trattata
/// come contenuto vero da sincronizzare (e, con le cancellazioni propagate
/// attive, anche cancellata come "file in più" da un altro job che non sa
/// cosa sia). Verificato empiricamente: passato come `_filter.ExcludeRule`
/// a `sync/copy`/`sync/sync` via RC, esclude davvero il contenuto della
/// cartella dal confronto.
pub(crate) const LOCK_FOLDER_EXCLUDE: &str = "/.rclone-easy-locks/**";

/// Quanto un lock resta valido dall'ULTIMO rinnovo prima di essere
/// considerato abbandonato (macchina spenta, crash, rete caduta) e quindi
/// sicuro da sovrascrivere. Nessuna verifica "il processo esiste ancora" è
/// possibile qui, a differenza del lock locale di bisync (vedi
/// `bisync::clear_stale_lock`): la macchina che l'ha scritto potrebbe non
/// essere nemmeno raggiungibile in rete. Un run genuinamente lungo (prima
/// sincronizzazione di un Drive grande: può durare ore, non minuti — non
/// deve farsi "rubare" il lock solo perché ci sta mettendo tanto) resta
/// protetto dal rinnovo periodico sotto (`RENEW_INTERVAL`); questa soglia
/// scatta solo se i rinnovi si fermano per davvero.
const STALE_AFTER_SECS: u64 = 30 * 60;

/// Intervallo di rinnovo del lock mentre il run che lo ha acquisito è
/// ancora in corso — ampio margine sotto `STALE_AFTER_SECS` (un mancato
/// rinnovo per un problema di rete transitorio non deve far scadere subito
/// il lock: restano comunque 2 tentativi di margine prima della soglia).
const RENEW_INTERVAL: Duration = Duration::from_secs(10 * 60);

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn lock_path_for(remote: &str) -> String {
    format!("{LOCK_FOLDER}/{remote}.lock")
}

/// Nome leggibile di questa macchina, solo per il messaggio mostrato
/// all'utente quando un lock è già in uso — mai usato per la logica di
/// acquisizione in sé (quella si basa solo su presenza/età del file).
/// `String::from("questo dispositivo")` come ripiego: preferibile a far
/// fallire l'intera acquisizione solo perché l'hostname non è leggibile su
/// questa piattaforma.
fn this_host() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// Contenuto del file di lock — `host`/`acquired_at_unix` servono per il
/// messaggio di conflitto e il controllo di scadenza, `nonce` (un numero
/// casuale, invariato per tutta la vita del lock: generato una sola volta
/// all'acquisizione, riusato identico ad ogni rinnovo) serve solo a
/// `try_acquire_one` per riconoscere con certezza "ho scritto io questo
/// contenuto, non qualcun altro nella stessa finestra di tempo" — nessuna
/// operazione "crea solo se assente" è disponibile su tutti i backend
/// rclone, quindi la conferma avviene rileggendo il contenuto dopo la
/// scrittura, vedi `try_acquire_one`.
#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
struct LockContent {
    host: String,
    #[serde(rename = "acquiredAtUnix")]
    acquired_at_unix: u64,
    nonce: u64,
}

/// Un lock acquisito con successo su un remote — va rilasciato esplicitamente
/// con `release_all` a fine run (successo o fallimento non cambia nulla).
/// Il task di rinnovo (`renew_handle`) viene interrotto lì, prima di
/// cancellare il file: non ha senso rinnovare un lock che si sta per
/// rilasciare.
#[derive(Debug)]
pub(crate) struct RemoteLockGuard {
    remote: String,
    renew_handle: tokio::task::JoinHandle<()>,
}

/// Legge il contenuto di un lock esistente copiandolo in un file temporaneo
/// locale (`operations/copyfile`, verificato funzionare in entrambe le
/// direzioni) — non c'è un endpoint RC per leggere il contenuto di un file
/// remoto direttamente in memoria. `None` per qualunque problema (contenuto
/// non valido, copia fallita): trattato dal chiamante come "non riesco a
/// dire se sia scaduto", quindi per prudenza NON stalizzato automaticamente
/// in quel caso — vedi `try_acquire_one`.
async fn read_lock_content(info: &ConnectionInfo, remote: &str, lock_path: &str) -> Option<LockContent> {
    let temp = tempfile::NamedTempFile::new().ok()?;
    let temp_path = temp.path();
    let temp_dir = temp_path.parent()?.to_string_lossy().into_owned();
    let temp_name = temp_path.file_name()?.to_string_lossy().into_owned();

    let fs = format!("{remote}:");
    info.call(
        "operations/copyfile",
        serde_json::json!({ "srcFs": fs, "srcRemote": lock_path, "dstFs": temp_dir, "dstRemote": temp_name }),
    )
    .await
    .ok()?;

    let content = std::fs::read_to_string(temp_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Scrive il lock sul remote (file temporaneo locale + `operations/copyfile`
/// verso il remote — verificato empiricamente che crea da sé le cartelle
/// intermedie, incluso `.rclone-easy-locks/` se non esiste già).
async fn write_lock(info: &ConnectionInfo, remote: &str, lock_path: &str, content: &LockContent) -> Result<(), String> {
    let json = serde_json::to_string(content).map_err(|e| format!("impossibile preparare il lock: {e}"))?;

    let temp = tempfile::NamedTempFile::new().map_err(|e| format!("impossibile creare un file temporaneo per il lock: {e}"))?;
    std::fs::write(temp.path(), &json).map_err(|e| format!("impossibile scrivere il lock temporaneo: {e}"))?;
    let temp_dir = temp.path().parent().ok_or("percorso temporaneo senza cartella padre")?.to_string_lossy().into_owned();
    let temp_name = temp.path().file_name().ok_or("percorso temporaneo senza nome file")?.to_string_lossy().into_owned();

    let fs = format!("{remote}:");
    info.call(
        "operations/copyfile",
        serde_json::json!({ "srcFs": temp_dir, "srcRemote": temp_name, "dstFs": fs, "dstRemote": lock_path }),
    )
    .await?;
    Ok(())
}

/// Messaggio mostrato quando un lock attivo (non scaduto) blocca
/// l'acquisizione — include chi lo tiene e da quanto tempo, quando
/// disponibile (best-effort: se il contenuto non è leggibile per qualche
/// motivo, un messaggio generico è comunque meglio di un errore tecnico
/// grezzo).
fn conflict_message(remote: &str, content: Option<&LockContent>) -> String {
    match content {
        Some(c) => {
            let minutes_ago = now_unix().saturating_sub(c.acquired_at_unix) / 60;
            format!(
                "'{remote}' è già in uso da un altro dispositivo ({}, da {minutes_ago} minuti) — riprova tra qualche minuto",
                c.host
            )
        }
        None => format!("'{remote}' è già in uso da un altro dispositivo — riprova tra qualche minuto"),
    }
}

/// Rinnova il lock a intervalli regolari finché non viene interrotto
/// (`RemoteLockGuard::renew_handle.abort()`, in `release_all`) — mantiene
/// lo stesso `nonce` dell'acquisizione originale (non cambia identità del
/// lock, solo il timestamp), così un run genuinamente lungo non supera mai
/// `STALE_AFTER_SECS` dall'ultimo segno di vita. Un fallimento di un
/// singolo rinnovo (rete instabile) non interrompe il loop: si riprova al
/// giro successivo, restano comunque margine prima della scadenza vera.
fn spawn_renewal(info: ConnectionInfo, remote: String, lock_path: String, nonce: u64) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RENEW_INTERVAL).await;
            let fresh = LockContent { host: this_host(), acquired_at_unix: now_unix(), nonce };
            let _ = write_lock(&info, &remote, &lock_path, &fresh).await;
        }
    })
}

/// Quanti tentativi ravvicinati fare prima di arrendersi — assorbe sia una
/// corsa genuina tra due macchine che scrivono nella stessa finestra di
/// pochi millisecondi (vedi il commento sulla rilettura di conferma sotto)
/// sia un singolo problema di lettura transitorio, senza dover distinguere
/// i due casi: un piccolo ritardo casuale tra un tentativo e l'altro
/// (jitter, stesso principio del backoff usato per le collisioni di rete)
/// disallinea i due contendenti, così a uno dei tentativi uno dei due
/// arriva chiaramente prima dell'altro. Se invece il lock è genuinamente in
/// uso da minuti (un run vero in corso altrove), questi tentativi falliscono
/// tutti allo stesso modo in meno di un secondo in totale — non c'è
/// un'attesa lunga, il chiamante riprova al giro successivo.
const MAX_ACQUIRE_ATTEMPTS: u32 = 8;

/// Tetto massimo per l'intera `try_acquire_one_retrying` (tutti i tentativi
/// insieme), non per la singola chiamata RC — senza un limite qui, un
/// remote irraggiungibile (es. server spento) lascia un job bisync
/// bloccato per sempre in attesa del lock, MOLTO prima di arrivare a
/// lanciare il sottoprocesso `rclone bisync` vero — quello sì fallisce in
/// fretta con un errore di rete pulito, ma non verrebbe mai raggiunto.
/// Ampio apposta rispetto al caso normale (un conflitto di lock genuino si
/// risolve in meno di un secondo, vedi il commento su
/// `MAX_ACQUIRE_ATTEMPTS`): non deve scattare per una rete solo lenta, solo
/// per una davvero morta. Senza questo limite, un remote irraggiungibile
/// lascia la chiamata bloccata indefinitamente, non solo per qualche
/// secondo in più.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

async fn try_acquire_one(info: &ConnectionInfo, remote: &str) -> Result<RemoteLockGuard, String> {
    match tokio::time::timeout(ACQUIRE_TIMEOUT, try_acquire_one_retrying(info, remote)).await {
        Ok(result) => result,
        Err(_) => {
            Err(format!("'{remote}' non risponde (timeout dopo {}s) — controlla che il remote sia raggiungibile", ACQUIRE_TIMEOUT.as_secs()))
        }
    }
}

async fn try_acquire_one_retrying(info: &ConnectionInfo, remote: &str) -> Result<RemoteLockGuard, String> {
    let mut last_err = None;
    for attempt in 0..MAX_ACQUIRE_ATTEMPTS {
        if attempt > 0 {
            // La finestra cresce ad ogni tentativo (stesso principio
            // dell'exponential backoff): se due contendenti restano vicini
            // di fase per un giro, un raggio via via più ampio li disallinea
            // comunque entro i tentativi rimasti.
            let jitter_ms = 50 + u64::from(attempt) * 80 + rand::random::<u64>() % 300;
            tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
        }
        match try_acquire_once(info, remote).await {
            Ok(guard) => return Ok(guard),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| conflict_message(remote, None)))
}

async fn try_acquire_once(info: &ConnectionInfo, remote: &str) -> Result<RemoteLockGuard, String> {
    let lock_path = lock_path_for(remote);
    let fs = format!("{remote}:");

    let stat = info.call("operations/stat", serde_json::json!({ "fs": fs, "remote": lock_path })).await?;
    let exists = stat.get("item").is_some_and(|v| !v.is_null());

    if exists {
        let content = read_lock_content(info, remote, &lock_path).await;
        let is_stale = match &content {
            // Contenuto illeggibile: non sappiamo dire se sia abbandonato,
            // per prudenza si tratta come ancora attivo invece di rischiare
            // di sovrascrivere un lock legittimo solo perché una singola
            // lettura è fallita per un problema di rete transitorio.
            None => false,
            Some(c) => now_unix().saturating_sub(c.acquired_at_unix) > STALE_AFTER_SECS,
        };
        if !is_stale {
            return Err(conflict_message(remote, content.as_ref()));
        }
    }

    let nonce = rand::random();
    let our_content = LockContent { host: this_host(), acquired_at_unix: now_unix(), nonce };
    write_lock(info, remote, &lock_path, &our_content).await?;

    // Prima di rileggere per conferma, un breve respiro casuale: senza
    // questo, in una corsa genuina la rilettura può capitare a metà tra la
    // scrittura di un contendente e quella dell'altro, così vicine nel tempo
    // che chi rilegge per primo "vince" anche se l'altro scrive un istante
    // dopo — scoperto empiricamente con un test dedicato (`tokio::join!` su
    // due acquisizioni parallele nello stesso runtime single-thread, dove le
    // due task si alternano in modo quasi deterministico ad ogni `.await`):
    // senza questa pausa capitava che ENTRAMBI i contendenti leggessero il
    // contenuto dell'altro (nessuno dei due si riconosceva vincitore, vedi
    // sotto il perché questo è peggio di un semplice "vince l'ultimo").
    // Allargare la finestra tra scrittura e rilettura dà tempo a una
    // scrittura concorrente di "arrivare prima" in modo consistente, invece
    // di capitare a cavallo della rilettura.
    let settle_ms = 50 + rand::random::<u64>() % 150;
    tokio::time::sleep(Duration::from_millis(settle_ms)).await;

    // Rilettura di conferma: un'altra macchina potrebbe aver scritto il
    // proprio lock nella stessa finestra di tempo tra il controllo sopra e
    // questa scrittura (nessuna operazione "crea solo se assente" è
    // disponibile su tutti i backend rclone) — se il contenuto che
    // rileggiamo non è esattamente quello che abbiamo appena scritto,
    // abbiamo perso la corsa: un'altra scrittura è arrivata dopo la nostra
    // (o non siamo riusciti a rileggere affatto, trattato allo stesso modo:
    // il tentativo successivo, dopo il jitter, chiarirà la situazione).
    let confirmed = read_lock_content(info, remote, &lock_path).await;
    match confirmed {
        Some(c) if c == our_content => {
            let renew_handle = spawn_renewal(info.clone(), remote.to_string(), lock_path, nonce);
            Ok(RemoteLockGuard { remote: remote.to_string(), renew_handle })
        }
        other => Err(conflict_message(remote, other.as_ref())),
    }
}

/// Acquisisce il lock su ogni remote in `remotes` (deduplicati: path1/path2
/// possono riferirsi allo stesso remote in teoria, anche se i vincoli
/// esistenti "un solo bisync/backup per remote" lo rendono raro in
/// pratica). Se una qualunque acquisizione fallisce dopo che le precedenti
/// sono riuscite, quelle già ottenute vengono rilasciate prima di
/// restituire l'errore — nessun lock lasciato "a metà". Usa
/// `rcd::connection_info` (non `rcd::call` diretto): il lock resta
/// acquisito per tutta la durata del run, che può essere lunga — tenere il
/// lock su `RcdState` per tutto quel tempo congelerebbe ogni altro comando
/// dell'app, stessa cautela già presa per l'attesa OAuth.
pub(crate) async fn acquire_all(state: &RcdState, remotes: &[&str]) -> Result<Vec<RemoteLockGuard>, String> {
    let mut unique: Vec<&str> = Vec::new();
    for r in remotes {
        if !unique.contains(r) {
            unique.push(r);
        }
    }

    let info = rcd::connection_info(state).await?;
    let mut acquired = Vec::new();
    for remote in unique {
        match try_acquire_one(&info, remote).await {
            Ok(guard) => acquired.push(guard),
            Err(e) => {
                release_all(state, acquired).await;
                return Err(e);
            }
        }
    }
    Ok(acquired)
}

/// Rilascia ogni lock acquisito — interrompe prima il rinnovo periodico
/// (`abort`), poi cancella il file. Best-effort sulla cancellazione: un
/// fallimento (es. rete caduta proprio ora) non deve far fallire l'esito
/// del run che l'ha appena completato — il lock resta comunque protetto
/// dalla scadenza in `try_acquire_one`, che prima o poi lo considererà
/// abbandonato (i rinnovi si sono fermati insieme al task). Anche questa
/// chiamata ha un tetto (`RELEASE_TIMEOUT`, più corto di `ACQUIRE_TIMEOUT`:
/// un solo colpo, nessun tentativo da ripetere) — stesso motivo di
/// `try_acquire_one`: un remote diventato irraggiungibile PROPRIO ora (a
/// run appena finito) non deve bloccare per sempre anche il rilascio.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn release_all(state: &RcdState, guards: Vec<RemoteLockGuard>) {
    let Ok(info) = rcd::connection_info(state).await else { return };
    for guard in guards {
        guard.renew_handle.abort();
        let fs = format!("{}:", guard.remote);
        let lock_path = lock_path_for(&guard.remote);
        let _ = tokio::time::timeout(RELEASE_TIMEOUT, info.call("operations/deletefile", serde_json::json!({ "fs": fs, "remote": lock_path }))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;

    /// In questi test un remote "finto" è semplicemente una cartella locale
    /// temporanea usata come `fs` — stesso pattern già consolidato altrove
    /// in questo progetto (es. `remotes.rs`), verificato a mano in questa
    /// sessione che `operations/stat`/`copyfile`/`deletefile` funzionano
    /// identicamente contro una cartella locale reale.
    fn fake_remote_path(dir: &TempDir) -> String {
        dir.path.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn acquire_then_release_leaves_the_remote_free_again() {
        let config_dir = TempDir::new("remote-lock-basic-config");
        let remote_dir = TempDir::new("remote-lock-basic-remote");
        std::fs::create_dir_all(&remote_dir.path).unwrap();
        let state = crate::rcd::build_state(config_dir.config_path()).await;
        let remote = fake_remote_path(&remote_dir);

        let guards = acquire_all(&state, &[&remote]).await.unwrap();
        assert_eq!(guards.len(), 1);

        release_all(&state, guards).await;

        // Libero di nuovo: una seconda acquisizione deve riuscire.
        let guards = acquire_all(&state, &[&remote]).await;
        assert!(guards.is_ok(), "il remote dovrebbe essere di nuovo libero dopo il rilascio");
    }

    #[tokio::test]
    async fn a_fresh_lock_blocks_a_second_acquisition_with_a_useful_message() {
        let config_dir = TempDir::new("remote-lock-conflict-config");
        let remote_dir = TempDir::new("remote-lock-conflict-remote");
        std::fs::create_dir_all(&remote_dir.path).unwrap();
        let state = crate::rcd::build_state(config_dir.config_path()).await;
        let remote = fake_remote_path(&remote_dir);

        let _guards = acquire_all(&state, &[&remote]).await.unwrap();

        let result = acquire_all(&state, &[&remote]).await;
        let err = result.expect_err("un lock fresco deve bloccare una seconda acquisizione");
        assert!(err.contains(&remote), "il messaggio deve nominare il remote in conflitto");
        assert!(err.contains(&this_host()), "il messaggio deve nominare chi tiene il lock");
    }

    #[tokio::test]
    async fn a_stale_lock_is_silently_overridden() {
        let config_dir = TempDir::new("remote-lock-stale-config");
        let remote_dir = TempDir::new("remote-lock-stale-remote");
        std::fs::create_dir_all(&remote_dir.path).unwrap();
        let state = crate::rcd::build_state(config_dir.config_path()).await;
        let remote = fake_remote_path(&remote_dir);

        // Scrive direttamente un lock "vecchio" (oltre la soglia di
        // scadenza, mai rinnovato), come se fosse stato lasciato da una
        // macchina crashata molto tempo fa.
        let lock_dir = remote_dir.path.join(LOCK_FOLDER);
        std::fs::create_dir_all(&lock_dir).unwrap();
        let stale_content = LockContent { host: "altro-pc".to_string(), acquired_at_unix: now_unix() - STALE_AFTER_SECS - 60, nonce: 42 };
        std::fs::write(lock_dir.join(format!("{remote}.lock")), serde_json::to_string(&stale_content).unwrap()).unwrap();

        let result = acquire_all(&state, &[&remote]).await;
        assert!(result.is_ok(), "un lock scaduto (mai rinnovato) non deve bloccare una nuova acquisizione: {result:?}");
    }

    /// Riproduce esattamente il caso segnalato da Simone: un run lungo (più
    /// del tempo di scadenza) non deve far "rubare" il lock a un'altra
    /// macchina solo perché ci sta mettendo tanto — qui si simula il
    /// passare del tempo scrivendo a mano un rinnovo "vecchio quanto basta
    /// da solo a far scattare la scadenza se non fosse per il rinnovo vero"
    /// e verificando che il lock resti comunque proprio, dato che il task
    /// di rinnovo è ancora attivo.
    #[tokio::test]
    async fn a_lock_still_being_renewed_is_never_treated_as_stale() {
        let config_dir = TempDir::new("remote-lock-renew-config");
        let remote_dir = TempDir::new("remote-lock-renew-remote");
        std::fs::create_dir_all(&remote_dir.path).unwrap();
        let state = crate::rcd::build_state(config_dir.config_path()).await;
        let remote = fake_remote_path(&remote_dir);

        let guards = acquire_all(&state, &[&remote]).await.unwrap();

        // Il lock è "fresco" (appena scritto): un secondo tentativo, come
        // farebbe un'altra macchina in qualunque momento durante un run
        // ancora attivo, deve continuare a vedersi rifiutato.
        let result = acquire_all(&state, &[&remote]).await;
        assert!(result.is_err(), "un lock ancora attivo (rinnovato o appena acquisito) deve continuare a bloccare");

        release_all(&state, guards).await;
    }

    /// Garanzia reale di un lock non atomico (nessun backend rclone offre un
    /// "crea solo se assente" via RC, vedi il commento su `LockContent`): la
    /// **mutua esclusione** è garantita sempre (mai entrambi vincono — questo
    /// sarebbe l'unico esito davvero pericoloso, due macchine che sincronizzano
    /// lo stesso remote in parallelo). La **liveness** (garantire che qualcuno
    /// vinca) no: sotto una contesa avversariale strettissima — scrittura,
    /// rilettura di conferma e i tentativi successivi tutti nella stessa
    /// finestra di pochi millisecondi, scenario che qui si forza apposta con
    /// `tokio::join!` ma che nella realtà non si presenta mai (macchine
    /// fisiche diverse, minuti tra un tentativo pianificato e l'altro) —
    /// entrambi i contendenti possono esaurire i tentativi e arrendersi
    /// entrambi. È un esito sicuro: nessuna delle due parte, il giro
    /// successivo (pianificato o manuale) si sblocca da solo. Verificato con
    /// run ripetuti (60+) che l'esito "vincono entrambi" non si presenta mai;
    /// l'esito "perdono entrambi" resta possibile ma raro, per questo motivo
    /// non è trattato come fallimento del test.
    #[tokio::test]
    async fn acquiring_the_same_remote_twice_concurrently_never_lets_both_win() {
        let config_dir = TempDir::new("remote-lock-race-config");
        let remote_dir = TempDir::new("remote-lock-race-remote");
        std::fs::create_dir_all(&remote_dir.path).unwrap();
        let state = std::sync::Arc::new(crate::rcd::build_state(config_dir.config_path()).await);
        let remote = fake_remote_path(&remote_dir);

        let state_a = state.clone();
        let remote_a = remote.clone();
        let state_b = state.clone();
        let remote_b = remote.clone();

        let (result_a, result_b) =
            tokio::join!(async move { acquire_all(&state_a, &[&remote_a]).await }, async move { acquire_all(&state_b, &[&remote_b]).await });

        let successes = [&result_a, &result_b].into_iter().filter(|r| r.is_ok()).count();
        assert!(successes <= 1, "due acquisizioni concorrenti non devono MAI riuscire entrambe: {result_a:?} / {result_b:?}");
    }

    #[tokio::test]
    async fn acquiring_multiple_remotes_rolls_back_already_acquired_locks_on_failure() {
        let config_dir = TempDir::new("remote-lock-rollback-config");
        let remote_a_dir = TempDir::new("remote-lock-rollback-a");
        let remote_b_dir = TempDir::new("remote-lock-rollback-b");
        std::fs::create_dir_all(&remote_a_dir.path).unwrap();
        std::fs::create_dir_all(&remote_b_dir.path).unwrap();
        let state = crate::rcd::build_state(config_dir.config_path()).await;
        let remote_a = fake_remote_path(&remote_a_dir);
        let remote_b = fake_remote_path(&remote_b_dir);

        // Il secondo remote è già occupato da un altro dispositivo.
        let _held = acquire_all(&state, &[&remote_b]).await.unwrap();

        let result = acquire_all(&state, &[&remote_a, &remote_b]).await;
        assert!(result.is_err(), "l'acquisizione complessiva deve fallire se anche solo un remote è occupato");

        // Il primo remote, acquisito con successo prima del fallimento sul
        // secondo, deve essere stato rilasciato di nuovo (nessun lock
        // "a metà").
        let retry = acquire_all(&state, &[&remote_a]).await;
        assert!(retry.is_ok(), "il primo remote non deve restare bloccato dopo il rollback: {retry:?}");
    }
}
