use crate::rcd::RcdState;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Intervallo del ciclo interno: abbastanza fine da non far aspettare un
/// job "ogni pochi minuti" più del previsto, senza sprecare cicli
/// controllando troppo spesso — l'intervallo minimo configurabile per un
/// job è comunque in minuti, non secondi.
const TICK: Duration = Duration::from_secs(30);

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `true` se un job con questo intervallo/ultima esecuzione va rilanciato
/// ora. Un job mai eseguito (`last_run_when_unix: None`) è sempre dovuto,
/// così un job automatico appena creato parte al primo giro del ciclo
/// invece di aspettare un intervallo intero a vuoto.
fn is_due(auto_interval_minutes: Option<u32>, last_run_when_unix: Option<u64>, now: u64) -> bool {
    let Some(interval) = auto_interval_minutes else { return false };
    match last_run_when_unix {
        None => true,
        Some(when) => now.saturating_sub(when) >= u64::from(interval) * 60,
    }
}

/// Avvia il ciclo interno che controlla periodicamente i job di sync e
/// bisync con un intervallo automatico configurato e li esegue quando è il
/// momento — alternativa più semplice a un systemd user timer esterno (per
/// ora scelta deliberatamente al posto di quanto descritto nello SPEC),
/// possibile perché l'app resta viva in background nella tray anche a
/// finestra chiusa (vedi `lib.rs::hide_instead_of_close`): non serve
/// nessuna comunicazione con un processo esterno. Si ferma solo se l'app
/// viene chiusa per davvero (voce "Esci" della tray, o un segnale diretto
/// al processo).
pub(crate) fn spawn(app: AppHandle, config_dir: PathBuf) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            run_due_jobs(&app, &config_dir).await;
        }
    });
}

/// Ogni job scaduto parte come task separato (non in sequenza): job
/// indipendenti tra loro devono poter procedere in parallelo, stesso
/// principio già valido per l'esecuzione manuale. Un fallimento silenzioso
/// qui (`let _ =`) è voluto — l'errore resta comunque visibile nella UI
/// tramite `last_run`, salvato da `run_job_by_name`/`run_bisync_job_by_name`
/// prima di restituirlo.
async fn run_due_jobs<R: tauri::Runtime>(app: &AppHandle<R>, config_dir: &Path) {
    let now = now_unix();

    if let Ok(jobs) = crate::jobs::load_from_dir(config_dir) {
        for job in jobs {
            let last_when = job.history.first().map(|r| r.when_unix);
            if !is_due(job.auto_interval_minutes, last_when, now) {
                continue;
            }
            // Cartella con attività locale molto recente: lascia che sia il
            // watcher a scattare non appena si stabilizza (watcher.rs),
            // invece di sincronizzare proprio mentre un file potrebbe
            // essere ancora in scrittura.
            if crate::watcher::is_backup_job_hot(&job.name) {
                continue;
            }
            let name = job.name.clone();
            let app = app.clone();
            let config_dir = config_dir.to_path_buf();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<RcdState>();
                let _ = crate::jobs::run_job_by_name(&config_dir, &state, &name).await;
            });
        }
    }

    if let Ok(jobs) = crate::bisync::load_from_dir(config_dir) {
        for job in jobs {
            let last_when = job.history.first().map(|r| r.when_unix);
            if !is_due(job.auto_interval_minutes, last_when, now) {
                continue;
            }
            if crate::watcher::is_bisync_job_hot(&job.name) {
                continue;
            }
            let name = job.name.clone();
            let app = app.clone();
            let config_dir = config_dir.to_path_buf();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<RcdState>();
                // Senza la password (quando la config è protetta,
                // config_password.rs), questo job automatico fallirebbe in
                // silenzio esattamente come il click manuale "Sincronizza
                // ora" — vedi il commento su `bisync::execute_bisync`.
                let password = crate::rcd::current_config_password(&state).await;
                let _ = crate::bisync::run_bisync_job_by_name(&config_dir, &state, password.as_deref(), &name).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_due_is_false_without_an_interval() {
        assert!(!is_due(None, None, 1000));
        assert!(!is_due(None, Some(0), 1000));
    }

    #[test]
    fn is_due_is_true_for_a_job_never_run() {
        assert!(is_due(Some(15), None, 1000));
    }

    #[test]
    fn is_due_respects_the_configured_interval() {
        let fifteen_minutes = 15 * 60;
        assert!(!is_due(Some(15), Some(1000), 1000 + fifteen_minutes - 1), "non ancora scaduto un secondo prima");
        assert!(is_due(Some(15), Some(1000), 1000 + fifteen_minutes), "scaduto esattamente all'intervallo");
        assert!(is_due(Some(15), Some(1000), 1000 + fifteen_minutes + 500), "scaduto ben oltre l'intervallo");
    }

    /// Un job scaduto per intervallo ma con attività locale molto recente
    /// (segnalata dal watcher, vedi `watcher.rs`) non deve partire da qui —
    /// altrimenti rischierebbe di sincronizzare un file ancora in
    /// scrittura proprio mentre il watcher sta per scattare da solo non
    /// appena si stabilizza.
    #[tokio::test]
    async fn run_due_jobs_skips_a_job_the_watcher_reports_as_hot() {
        let dir = crate::rcd::tests::TempDir::new("scheduler-hot-skip");
        let source_dir = crate::rcd::tests::TempDir::new("scheduler-hot-skip-source");
        let dest_dir = crate::rcd::tests::TempDir::new("scheduler-hot-skip-dest");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&dest_dir.path).unwrap();

        crate::jobs::create_job_in(
            &dir.path,
            "prova-hot",
            &source_dir.path.to_string_lossy(),
            &dest_dir.path.to_string_lossy(),
            Some(1),
            false,
        )
        .unwrap();
        crate::watcher::mark_hot_for_test(true, "prova-hot");

        let mock = tauri::test::mock_app();
        let app = mock.handle().clone();
        run_due_jobs(&app, &dir.path).await;

        // Se il job fosse partito nonostante il segnale "caldo", il task
        // spawnato separatamente avrebbe avuto tempo di completarsi e
        // salvare la history entro questo margine.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let jobs = crate::jobs::load_from_dir(&dir.path).unwrap();
        assert!(jobs[0].history.is_empty(), "il job segnalato come caldo non dovrebbe essere partito");
    }
}
