use tauri::Manager;

mod fs_atomic;

mod existing_config;
use existing_config::{detect_existing_remotes, standard_rclone_config_path};

mod rclone_bin;
use rclone_bin::check_rclone_installed;

mod rcd;
use rcd::{rcd_status, RcdState};

mod activity;

mod remotes;
use remotes::{
    create_remote, delete_remote, delete_remote_cascade, get_provider_options, get_remote_for_edit, import_remote,
    list_own_remotes, list_provider_types, list_remote_dir, list_s3_endpoints, list_s3_providers, list_s3_regions,
    own_config_path, update_remote,
};

mod interactive_remote;
use interactive_remote::{answer_oauth_question, cancel_oauth, create_remote_interactive, PendingOAuthAnswer};

mod onedrive_recovery;

mod tray;
use tray::hide_window;

mod app_settings;
use app_settings::{get_app_settings, set_start_minimized};

mod open_external;
use open_external::{open_local_folder, open_url_in_browser};

mod installation;
use installation::installation_kind;

mod update_state;
use update_state::{report_update_available, UpdateState};

// xdg-desktop-portal è un concetto specifico dei desktop Linux (D-Bus) —
// non ha senso su Windows/macOS, dove tra l'altro non esiste un bus di
// sessione a cui connettersi (fallirebbe silenziosamente ad ogni avvio,
// innocuo ma inutile). Escluso anche dalla compilazione, non solo
// dall'esecuzione: evita di trascinare la dipendenza `zbus` su piattaforme
// dove non serve a nulla.
#[cfg(target_os = "linux")]
mod background_portal;

#[cfg(target_os = "linux")]
mod desktop_integration;

mod remote_lock;

mod trash;
use trash::{list_trash, restore_trash_entry};

mod duplicates;
use duplicates::{delete_duplicate, delete_review_entry, keep_all_duplicates, list_duplicate_group, list_review_entries, move_duplicate_for_review, open_local_duplicate};

mod path_safety;
use path_safety::check_dangerous_path;

mod jobs;
use jobs::{create_job, delete_job, dry_run_job, list_jobs, run_job, update_job};

mod mounts;
use mounts::{
    create_mount, delete_mount, download_and_launch_winfsp_installer, list_mounts, mount_now, mount_now_and_open,
    open_mount_folder, unmount_now, update_mount,
};

mod bisync;
use bisync::{
    create_bisync_job, delete_bisync_job, dry_run_bisync_job, list_bisync_jobs, run_bisync_job, run_bisync_job_forced,
    update_bisync_job,
};

mod scheduler;
mod watcher;

mod backup;
use backup::{export_backup, import_backup};

mod config_password;
use config_password::{config_password_status, needs_unlock, remove_config_password, set_config_password, unlock_config, PASSWORD_HELPER_ARG};

/// Riavvia l'app — usata dopo un ripristino di backup, che sovrascrive i
/// file di config sotto `app_config_dir()` senza far ripartire da sé il
/// demone rcd né i mount/job già in corso (vedi `backup::import_backup`).
/// `request_restart` (non il semplice `restart`, che sui thread giusti può
/// saltare la consegna di `RunEvent::Exit`) passa comunque dall'handler in
/// `run()` sotto, che spegne il demone rclone rcd in modo pulito prima di
/// far ripartire il processo.
#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    app.request_restart();
}

/// Mostrata una sola volta per sessione dell'app (non persistita tra
/// riavvii: non esiste ancora un sistema di impostazioni dell'app oltre al
/// file rclone.conf) la prima volta che l'utente chiude la finestra e
/// questa si nasconde invece di uscire — altrimenti un'utente non tecnica
/// potrebbe credere di aver chiuso l'app per sbaglio.
static TRAY_NOTICE_SHOWN: std::sync::Once = std::sync::Once::new();

/// Chiudere la finestra (X) la nasconde invece di terminare l'app: è quello
/// che rende utile un'icona "persistente" nella tray (stesso comportamento
/// di client come Dropbox/Insync) — il demone rclone e i futuri job restano
/// attivi, si esce davvero solo dalla voce "Esci" del menu della tray
/// (`tray::build_tray`).
fn hide_instead_of_close(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window(tray::MAIN_WINDOW_LABEL) else { return };
    let app_handle = app.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            tray::hide_main_window(&app_handle);
            TRAY_NOTICE_SHOWN.call_once(|| {
                // Thread OS a parte, non il thread/task chiamante: stessa
                // cautela di tray.rs::notify_error sul crash "Cannot start a
                // runtime from within a runtime" del plugin di notifica —
                // qui probabilmente innocuo (questo handler non gira su un
                // worker tokio), ma costa nulla essere coerenti ovunque si
                // chiami questa API.
                let app_handle = app_handle.clone();
                std::thread::spawn(move || {
                    use tauri_plugin_notification::NotificationExt;
                    let _ = app_handle
                        .notification()
                        .builder()
                        .title("Rclone Easy")
                        .body(
                            "L'app continua a funzionare in background. Trovi l'icona nella tray di sistema; usa \"Esci\" dal suo menu per chiuderla davvero.",
                        )
                        .show();
                });
            });
        }
    });
}

/// `RunEvent::Exit` (agganciato più sotto) scatta solo quando l'app si
/// chiude "normalmente" attraverso il ciclo di eventi della finestra. Un
/// segnale diretto al processo — verificato durante lo sviluppo: capita ad
/// ogni riavvio a caldo di `npm run tauri dev` quando si modifica un file
/// Rust, e capiterebbe anche a un utente che spegne/disconnette la sessione
/// con l'app aperta — non passa da lì, quindi `rclone rcd` restava orfano.
/// Installare qui un gestore per SIGTERM/SIGINT che fa la stessa pulizia
/// esplicita prima di uscire chiude anche questo caso. Solo Unix (Linux/macOS,
/// coerente con la fase attuale solo-Linux del progetto): Windows userà
/// `tokio::signal::windows` quando arriverà quella fase.
#[cfg(unix)]
fn spawn_signal_shutdown_handler(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("impossibile installare il gestore SIGTERM");
        let mut sigint = signal(SignalKind::interrupt()).expect("impossibile installare il gestore SIGINT");
        tokio::select! {
            _ = sigterm.recv() => {}
            _ = sigint.recv() => {}
        }
        // Bisync gira come sottoprocesso diretto, non attraverso rclone rcd:
        // spegnere solo il demone lo lascerebbe orfano in background, con il
        // suo lock file mai rilasciato — vedi `bisync::terminate_running_bisyncs`.
        bisync::terminate_running_bisyncs();
        let state = app.state::<RcdState>();
        rcd::shutdown(&state).await;
        std::process::exit(0);
    });
}

/// Punto d'ingresso vero e proprio (`main.rs`): prima di avviare l'app
/// Tauri, intercetta la modalità "aiutante" usata come `--password-command`
/// da `rclone config encryption set/remove` (`config_password.rs`) — se
/// invocato così, stampa la password su stdout ed esce subito, senza mai
/// avviare una seconda istanza dell'app vera. Evita di dover scrivere la
/// password su un file temporaneo solo per passarla a un sottoprocesso.
pub fn run_or_password_helper() {
    if std::env::args().nth(1).as_deref() == Some(PASSWORD_HELPER_ARG) {
        config_password::print_helper_password();
        return;
    }
    run();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Deve essere il primo plugin registrato (raccomandazione di Tauri):
        // rileva una seconda istanza già all'avvio, prima che qualunque
        // altra cosa (demone rcd, tray) parta per davvero in quella seconda
        // copia. Senza questo, avviare l'app due volte produce due processi
        // completi e due icone nella tray, perché nessun controllo
        // impedirebbe una seconda istanza. La callback gira nella PRIMA
        // istanza quando ne viene lanciata una seconda: si limita a
        // riportare in primo piano la finestra già esistente, stesso
        // comportamento dell'icona della tray.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        // `LaunchAgent` è l'unica variante rilevante fuori da macOS (lì
        // sarebbe scelta tra LaunchAgent/LaunchDaemon); su Linux/Windows il
        // plugin usa comunque il meccanismo nativo della piattaforma (voce
        // XDG autostart / registro Run), il parametro è semplicemente
        // ignorato. Nessun argomento extra passato al riavvio automatico:
        // l'app si comporta come un avvio normale, "ridotta a icona" è una
        // preferenza propria (`app_settings.rs`) letta comunque a runtime.
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Config propria dell'app, separata dal rclone.conf di sistema
            // (letto invece, in sola lettura, da existing_config.rs) — vedi
            // SPEC.md, sezione "Configurazione".
            let config_dir = app
                .path()
                .app_config_dir()
                .expect("impossibile determinare la cartella di configurazione dell'app");
            let config_path = config_dir.join("rclone.conf");
            // Config protetta da password (config_password.rs): non avviare
            // il demone adesso, resta nello stato segnaposto finché la UI
            // non manda la password giusta tramite `unlock_config` — vedi
            // `needs_unlock`/`unlock` in rcd.rs.
            let rcd_state: RcdState = if rcd::config_is_encrypted(&config_path) {
                rcd::locked_state()
            } else {
                tauri::async_runtime::block_on(rcd::build_state(config_path))
            };
            app.manage(rcd_state);
            app.manage(PendingOAuthAnswer::default());
            app.manage(UpdateState::default());
            #[cfg(unix)]
            spawn_signal_shutdown_handler(app.handle().clone());
            tray::build_tray(app.handle());
            hide_instead_of_close(app.handle());
            // La finestra parte nascosta (`tauri.conf.json`, `visible:
            // false`) per evitare un lampo "appare e poi sparisce subito"
            // quando l'utente ha scelto di avviare ridotta a icona — la si
            // mostra qui esplicitamente solo se quella preferenza non è
            // attiva.
            if !app_settings::load_from_dir(&config_dir).start_minimized {
                tray::show_main_window(app.handle());
            }
            #[cfg(target_os = "linux")]
            background_portal::request_background();
            #[cfg(target_os = "linux")]
            desktop_integration::ensure_installed();
            // In background, non bloccante: montare subito i mount con
            // auto_mount potrebbe richiedere secondi (connessione a un
            // remote cloud), non deve ritardare la comparsa della finestra.
            {
                let app_handle = app.handle().clone();
                let config_dir = config_dir.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app_handle.state::<RcdState>();
                    mounts::auto_mount_all(&state, &config_dir).await;
                });
            }
            // Ciclo interno per i job di sync/bisync con esecuzione
            // automatica configurata — vedi scheduler.rs sul perché non un
            // systemd user timer esterno.
            scheduler::spawn(app.handle().clone(), config_dir.clone());
            // Osserva le cartelle locali di backup/bisync con automazione
            // attiva, per anticipare la sincronizzazione rispetto al solo
            // intervallo fisso — vedi watcher.rs.
            watcher::spawn(app.handle().clone(), config_dir.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            check_rclone_installed,
            detect_existing_remotes,
            standard_rclone_config_path,
            rcd_status,
            list_own_remotes,
            list_remote_dir,
            create_remote,
            update_remote,
            delete_remote,
            delete_remote_cascade,
            get_remote_for_edit,
            list_s3_providers,
            list_s3_regions,
            list_s3_endpoints,
            list_provider_types,
            get_provider_options,
            own_config_path,
            import_remote,
            create_remote_interactive,
            cancel_oauth,
            answer_oauth_question,
            list_jobs,
            create_job,
            update_job,
            delete_job,
            run_job,
            dry_run_job,
            list_mounts,
            create_mount,
            update_mount,
            delete_mount,
            mount_now,
            mount_now_and_open,
            open_mount_folder,
            unmount_now,
            download_and_launch_winfsp_installer,
            list_bisync_jobs,
            create_bisync_job,
            update_bisync_job,
            delete_bisync_job,
            run_bisync_job,
            run_bisync_job_forced,
            dry_run_bisync_job,
            list_trash,
            restore_trash_entry,
            list_duplicate_group,
            move_duplicate_for_review,
            delete_duplicate,
            open_local_duplicate,
            list_review_entries,
            delete_review_entry,
            keep_all_duplicates,
            check_dangerous_path,
            export_backup,
            import_backup,
            restart_app,
            needs_unlock,
            unlock_config,
            config_password_status,
            set_config_password,
            remove_config_password,
            hide_window,
            get_app_settings,
            set_start_minimized,
            open_url_in_browser,
            open_local_folder,
            installation_kind,
            report_update_available
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Terminazione esplicita del demone rclone rcd: kill_on_drop da
            // solo non è affidabile perché Tauri non garantisce che il Drop
            // dello stato giri alla chiusura normale dell'app (verificato
            // empiricamente durante lo sviluppo).
            if let tauri::RunEvent::Exit = event {
                let state = app_handle.state::<RcdState>();
                tauri::async_runtime::block_on(rcd::shutdown(&state));
            }
        });
}
