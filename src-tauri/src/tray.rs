use crate::rcd::RcdState;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tauri::{
    image::Image,
    menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Emitter, Manager, Wry,
};

const SHOW_HIDE_ID: &str = "show_hide";
const SETTINGS_ID: &str = "settings";
const QUIT_ID: &str = "quit";
const MOUNT_PREFIX: &str = "mount:";
const UNMOUNT_PREFIX: &str = "unmount:";
const BACKUP_PREFIX: &str = "backup:";
const BISYNC_PREFIX: &str = "bisync:";
/// Voce mostrata per un remote senza alcun mount/job/bisync configurato —
/// porta l'utente alla riga di quel remote in home invece di offrire azioni
/// che non esistono ancora, vedi `build_remote_submenu`.
const CONFIGURE_PREFIX: &str = "configure:";
/// Voce mostrata in cima al menu per ogni remote la cui ultima esecuzione
/// registrata (backup o bisync) è fallita — vedi `collect_remote_actions` e
/// `build_menu_from_remotes`.
const WARNING_PREFIX: &str = "warning:";
/// Evento ascoltato da `RemoteRow.svelte`: porta la finestra in primo piano
/// e naviga alla pagina del remote indicato nel payload (`/remote/[name]`),
/// aprendola sulla Cronologia se il click veniva da una voce di avviso.
/// Stesso schema di nome (`rclone-easy://...`) degli altri eventi verso il
/// frontend, vedi `oauth_remote.rs`.
const FOCUS_REMOTE_EVENT: &str = "rclone-easy://tray-focus-remote";
/// Evento ascoltato da `SettingsButton.svelte`: apre il modal Impostazioni —
/// stesso schema di `FOCUS_REMOTE_EVENT` per la voce "Impostazioni" del menu
/// della tray, vedi `open_settings`.
const OPEN_SETTINGS_EVENT: &str = "rclone-easy://open-settings";
/// Evento ascoltato da `UpdateButton.svelte`: apre direttamente il modal di
/// aggiornamento — emesso sia dalla voce di menu dedicata sia da
/// "Mostra/Nascondi finestra" quando c'è un aggiornamento in sospeso, vedi
/// `UPDATE_ID`/`toggle_main_window`.
const OPEN_UPDATE_EVENT: &str = "rclone-easy://open-update";
/// Voce di menu per l'aggiornamento in sospeso — mostrata solo quando
/// `update_state::UpdateState` ne segnala uno, vedi `build_menu_from_remotes`.
const UPDATE_ID: &str = "update_available";

/// Nome della finestra principale, quello di default assegnato da Tauri
/// quando `tauri.conf.json` non specifica un `label` esplicito (confermato:
/// non lo specifica).
pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

/// Icona unica colorata (non una coppia chiaro/scuro): niente rilevamento a
/// runtime del tema del pannello, che su Linux non ha un'API affidabile e
/// uniforme tra desktop environment diversi — scelta esplicita dell'utente.
///
/// Pixel RGBA grezzi (non PNG): `tauri::image::Image` in questa versione
/// non decodifica PNG da sé (`Image::new` vuole già RGBA piano), quindi i
/// file sono pre-decodificati una volta con Pillow da `icons/tray/tray-*.png`
/// a `icons/tray/tray-*.rgba`, 64×64. Se le sorgenti PNG cambiano vanno
/// rigenerati allo stesso modo.
const TRAY_ICON_SIZE: u32 = 64;
const TRAY_FRAME_BYTE_LEN: usize = (TRAY_ICON_SIZE * TRAY_ICON_SIZE * 4) as usize;
const IDLE_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/tray-idle.rgba");
/// Sequenza di fotogrammi del logo che ruota su se stesso di 360°, mostrata
/// in loop mentre almeno un job di backup o bisync è in esecuzione — vedi
/// `watch_activity`. Preferita a una dissolvenza di colore teal→ambra: un
/// cambio di colore a colpo d'occhio in una tray si nota meno di un
/// movimento. Usa la stessa API multipiattaforma di
/// Tauri per l'icona della tray (nessun trucco specifico per Linux), quindi
/// la stessa affidabilità vale anche per Windows/macOS in futuro. 24
/// fotogrammi (rotazione di 15° l'uno) generati una volta con Pillow da
/// `icons/tray/tray-idle.png`, ruotando su un canvas allargato a
/// dimensione-diagonale per non tagliare gli angoli del logo, poi
/// ridotti a 64×64.
const SPIN_FRAME_BYTES: &[u8] = include_bytes!("../icons/tray/tray-spin.rgba");
/// Dissolvenza morbida ambra→rosso acceso ("respiro", stessa formula di
/// easing a coseno per una transizione più naturale agli estremi che a
/// metà) — mostrata in loop al posto dell'icona idle statica quando
/// l'ultima esecuzione registrata di un job di backup o bisync (qualunque
/// remote) è fallita, finché non ne va a buon fine una successiva. Un
/// badge statico era troppo poco visibile nella tray a dimensione reale;
/// un cambio di colore persistente si nota molto di più. A differenza
/// dello stato "in esecuzione" sopra, qui resta un cambio di colore e non
/// una rotazione: è uno stato persistente (può durare ore), non un'attesa
/// attiva, quindi non deve sembrare "in corso qualcosa adesso".
const ERROR_BREATHE_FRAME_BYTES: &[u8] = include_bytes!("../icons/tray/tray-error-breathe.rgba");
/// Dissolvenza morbida teal→ambra→teal (mai fino al rosso, a differenza
/// dell'animazione di errore sopra: un aggiornamento disponibile non è un
/// problema, non deve sembrare allarmante) — mostrata in loop quando c'è un
/// aggiornamento in sospeso non ancora saltato dall'utente (vedi
/// `update_state::UpdateState`), finché non lo installa o lo salta. Stessa
/// tecnica ("respiro" a easing di coseno) e stessi 24 fotogrammi generati
/// una volta con Pillow da `icons/tray/tray-idle.png`, colore di partenza
/// idle (42,148,174) e di picco ambra (245,158,11) — lo stesso ambra usato
/// come primo fotogramma dell'animazione di errore sopra.
const UPDATE_BREATHE_FRAME_BYTES: &[u8] = include_bytes!("../icons/tray/tray-update-breathe.rgba");
/// Conteggio fotogrammi condiviso da entrambe le animazioni sopra (rotazione
/// e respiro), stesso valore per costruzione.
const ANIMATION_FRAME_COUNT: usize = 24;
/// Intervallo tra un fotogramma e il successivo, condiviso dalle due
/// animazioni: abbastanza fitto da sembrare fluido, abbastanza largo da
/// restare affidabile su qualunque implementazione di tray/menu bar. Per la
/// rotazione, un giro completo dura quindi 24 × 120ms ≈ 2,9s.
const ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(120);
/// Intervallo di controllo quando non c'è nulla in esecuzione — nessuna
/// icona da aggiornare in quella finestra, un controllo più rado va bene.
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Intervallo di ricostruzione del menu — vedi `watch_menu`.
const MENU_REFRESH_INTERVAL: Duration = Duration::from_secs(3);

/// Handle salvato come stato gestito da Tauri per poterne cambiare icona e
/// menu più avanti (`watch_activity`/`watch_menu`) — `TrayIconBuilder::build`
/// restituisce l'handle una volta sola, va conservato da qualche parte per
/// riusarlo.
struct TrayHandle(TrayIcon);

/// Icona persistente nella tray richiesta dallo SPEC, con menu contestuale
/// che elenca i remote configurati e le loro azioni rapide (monta/smonta,
/// backup ora, sincronizza ora) e stato "in esecuzione"/"ultimo fallito"
/// riflesso nell'icona — vedi `watch_activity`/`watch_menu`.
///
/// Non fallisce l'avvio dell'app se la tray non può essere creata (es.
/// nessun systray disponibile sul desktop dell'utente finale, scenario
/// comune con alcuni window manager Linux minimali o GNOME senza
/// l'estensione AppIndicator) — l'app resta comunque utilizzabile come
/// finestra normale.
pub fn build_tray(app: &AppHandle) {
    if let Err(e) = try_build_tray(app) {
        eprintln!("impossibile creare l'icona nella tray, l'app resta comunque utilizzabile: {e}");
    }
}

fn try_build_tray(app: &AppHandle) -> tauri::Result<()> {
    // Placeholder minimo: il primo giro di `watch_menu`, praticamente
    // immediato, lo sostituisce con la versione completa (coinvolge una
    // chiamata RC per lo stato live dei mount, quindi va costruito in modo
    // asincrono — non blocca la comparsa della tray).
    let show_hide = MenuItem::with_id(app, SHOW_HIDE_ID, "Mostra/Nascondi finestra", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, SETTINGS_ID, "Impostazioni", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, QUIT_ID, "Esci", true, None::<&str>)?;
    let placeholder_menu = Menu::with_items(app, &[&show_hide, &settings, &separator, &quit])?;

    let idle_icon = Image::new(IDLE_ICON_BYTES, TRAY_ICON_SIZE, TRAY_ICON_SIZE);

    let tray = TrayIconBuilder::new()
        .icon(idle_icon)
        .tooltip(TOOLTIP_OK)
        .menu(&placeholder_menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id.as_ref();
            if let Some(remote) = id.strip_prefix(CONFIGURE_PREFIX) {
                focus_remote(app, remote, false);
                return;
            }
            if let Some(remote) = id.strip_prefix(WARNING_PREFIX) {
                focus_remote(app, remote, true);
                return;
            }
            match id {
                SHOW_HIDE_ID => toggle_main_window(app),
                SETTINGS_ID => open_settings(app),
                UPDATE_ID => open_update(app),
                QUIT_ID => perform_quit(app),
                _ => dispatch_action(app, id),
            }
        })
        .build(app)?;

    app.manage(TrayHandle(tray));

    let activity_handle = app.clone();
    tauri::async_runtime::spawn(async move { watch_activity(activity_handle).await });

    let menu_handle = app.clone();
    tauri::async_runtime::spawn(async move { watch_menu(menu_handle).await });

    Ok(())
}

fn set_tray_icon(app: &AppHandle, bytes: &[u8]) {
    let Some(tray) = app.try_state::<TrayHandle>() else { return };
    let icon = Image::new(bytes, TRAY_ICON_SIZE, TRAY_ICON_SIZE);
    let _ = tray.0.set_icon(Some(icon));
}

fn set_tray_tooltip(app: &AppHandle, text: &str) {
    let Some(tray) = app.try_state::<TrayHandle>() else { return };
    let _ = tray.0.set_tooltip(Some(text));
}

fn animation_frame(frames: &[u8], frame: usize) -> &[u8] {
    let start = frame * TRAY_FRAME_BYTE_LEN;
    &frames[start..start + TRAY_FRAME_BYTE_LEN]
}

fn any_job_running() -> bool {
    crate::jobs::any_job_running() || crate::bisync::any_job_running()
}

/// `true` se l'ultima esecuzione registrata di un qualunque job di backup o
/// bisync (qualunque remote) è fallita — pilota l'animazione di errore
/// sull'icona quando l'app torna inattiva. Best-effort: un fallimento nel
/// leggere la config (es. cartella non ancora creata al primissimo avvio)
/// conta come "nessun errore" invece di far fallire il controllo.
fn last_run_failed(app: &AppHandle) -> bool {
    let Ok(config_dir) = app.path().app_config_dir() else { return false };

    let jobs_failed = crate::jobs::load_from_dir(&config_dir)
        .map(|jobs| jobs.iter().any(|j| j.history.first().is_some_and(|h| !h.success)))
        .unwrap_or(false);
    let bisync_failed = crate::bisync::load_from_dir(&config_dir)
        .map(|jobs| jobs.iter().any(|j| j.history.first().is_some_and(|h| !h.success)))
        .unwrap_or(false);

    jobs_failed || bisync_failed
}

#[derive(PartialEq, Clone, Copy)]
enum ActivityState {
    Running,
    Error,
    UpdateAvailable,
    Ok,
}

const TOOLTIP_RUNNING: &str = "Rclone Easy - Sincronizzazione in corso";
/// Testo del tooltip usato anche per capire, a colpo d'occhio nel codice,
/// cosa comunica l'animazione ambra→rosso di `ERROR_BREATHE_FRAME_BYTES`.
const TOOLTIP_ERROR: &str = "Rclone Easy - Uno o più lavori hanno avuto un problema";
const TOOLTIP_UPDATE: &str = "Rclone Easy - È disponibile un aggiornamento";
const TOOLTIP_OK: &str = "Rclone Easy - Ultimi lavori eseguiti con successo";

/// `true` se c'è un aggiornamento in sospeso non ancora saltato
/// dall'utente — vedi `update_state::UpdateState`.
fn pending_update(app: &AppHandle) -> Option<String> {
    app.try_state::<crate::update_state::UpdateState>().and_then(|s| s.pending_version())
}

/// Ogni ~120ms, mentre almeno un job è in esecuzione, mentre l'ultima
/// esecuzione registrata di un job è fallita, o mentre c'è un aggiornamento
/// in sospeso, mostra il fotogramma successivo della relativa animazione
/// (rotazione per "in corso", respiro ambra→rosso per "problema", respiro
/// teal→ambra per "aggiornamento disponibile"); appena torna tutto normale
/// mostra l'icona statica idle, controllato ogni ~500ms. Ordine di priorità
/// quando più condizioni sono vere insieme: un job in corso o un errore
/// contano più di un semplice avviso di aggiornamento, che può aspettare.
/// Il tooltip riflette lo stesso stato, aggiornato solo al cambio di stato
/// (non ad ogni fotogramma) per non chiamare l'API della tray decine di
/// volte al secondo inutilmente. Un polling semplice invece di un canale di
/// notifica: evita di dover far conoscere un `AppHandle` a
/// `jobs.rs`/`bisync.rs`, che oggi restano testabili senza una vera app
/// Tauri.
async fn watch_activity(app: AppHandle) {
    let mut current_state: Option<ActivityState> = None;
    let mut frame = 0usize;

    loop {
        let state = if any_job_running() {
            ActivityState::Running
        } else if last_run_failed(&app) {
            ActivityState::Error
        } else if pending_update(&app).is_some() {
            ActivityState::UpdateAvailable
        } else {
            ActivityState::Ok
        };

        if current_state != Some(state) {
            let tooltip = match state {
                ActivityState::Running => TOOLTIP_RUNNING,
                ActivityState::Error => TOOLTIP_ERROR,
                ActivityState::UpdateAvailable => TOOLTIP_UPDATE,
                ActivityState::Ok => TOOLTIP_OK,
            };
            set_tray_tooltip(&app, tooltip);
            current_state = Some(state);
            frame = 0;
        }

        match state {
            ActivityState::Running => {
                set_tray_icon(&app, animation_frame(SPIN_FRAME_BYTES, frame));
                frame = (frame + 1) % ANIMATION_FRAME_COUNT;
                tokio::time::sleep(ANIMATION_FRAME_INTERVAL).await;
            }
            ActivityState::Error => {
                set_tray_icon(&app, animation_frame(ERROR_BREATHE_FRAME_BYTES, frame));
                frame = (frame + 1) % ANIMATION_FRAME_COUNT;
                tokio::time::sleep(ANIMATION_FRAME_INTERVAL).await;
            }
            ActivityState::UpdateAvailable => {
                set_tray_icon(&app, animation_frame(UPDATE_BREATHE_FRAME_BYTES, frame));
                frame = (frame + 1) % ANIMATION_FRAME_COUNT;
                tokio::time::sleep(ANIMATION_FRAME_INTERVAL).await;
            }
            ActivityState::Ok => {
                set_tray_icon(&app, IDLE_ICON_BYTES);
                tokio::time::sleep(IDLE_POLL_INTERVAL).await;
            }
        }
    }
}

/// Stato di un job (backup o bisync) rilevante per il menu della tray: nome
/// per costruire l'id della voce, se ha la sincronizzazione automatica
/// accesa (`autoIntervalMinutes` impostato — stesso criterio di
/// `activity::active_service_for_remote`, NON "sta girando in questo
/// istante": è quello che il backend controlla davvero in `mounts::mount_now`
/// prima di montare, quindi deve essere anche il gate per "Monta" qui, pena
/// mostrarlo abilitato e farlo comunque fallire al click) e se la sua
/// ultima esecuzione registrata è fallita (voce di avviso in cima al menu).
struct JobStatus {
    name: String,
    auto_active: bool,
    last_failed: bool,
}

/// Azioni rapide disponibili per un remote nel menu — al più un mount, un
/// backup e una bisync per remote (stesso vincolo "uno per tipo" già
/// applicato altrove), quindi al più tre voci nel suo sottomenu. Un remote
/// con tutti e tre i campi a `None` non ha ancora nulla di configurato:
/// `build_remote_submenu` gli mostra "Configura" al posto delle azioni.
#[derive(Default)]
struct RemoteActions {
    mount: Option<(String, bool)>,
    backup: Option<JobStatus>,
    bisync: Option<JobStatus>,
}

impl RemoteActions {
    fn is_empty(&self) -> bool {
        self.mount.is_none() && self.backup.is_none() && self.bisync.is_none()
    }
}

/// Nome del remote referenziato da una stringa `fs` (`remoto:percorso`),
/// `None` se è un percorso locale — riconosciuto tramite `Path::is_absolute`,
/// non dal solo `/` iniziale: su Windows un percorso come `C:\Users\...` è
/// assoluto ma non inizia per `/`, e col vecchio controllo veniva scambiato
/// per un remote chiamato "C" (appariva una voce fantasma "C" nel menu
/// della tray, mentre il remote vero restante dietro l'`.or_else` non
/// veniva mai raggiunto). Stessa funzione duplicata in
/// `mounts.rs`/`jobs.rs`/`bisync.rs`/`activity.rs`: qui serve una copia
/// propria per non introdurre una dipendenza incrociata.
fn remote_name_of(fs: &str) -> Option<&str> {
    if Path::new(fs).is_absolute() {
        return None;
    }
    fs.split_once(':').map(|(name, _)| name)
}

async fn collect_remote_actions(app: &AppHandle, config_dir: &Path) -> Vec<(String, RemoteActions)> {
    let mut map: HashMap<String, RemoteActions> = HashMap::new();

    // Tutti i remote esistenti (non solo quelli con un mount/job/bisync già
    // configurato), così anche i remote "nudi" compaiono nel menu con la
    // voce "Configura" invece di essere semplicemente assenti — vedi
    // `build_remote_submenu`. Best-effort: se rcd non risponde (es. app
    // appena avviata) i remote restano comunque elencabili tramite mount/
    // job/bisync già salvati sotto, solo senza le voci "Configura" per gli
    // altri.
    if let Some(state) = app.try_state::<RcdState>() {
        if let Ok(body) = crate::rcd::call(&state, "config/listremotes", serde_json::json!({})).await {
            if let Ok(names) = crate::remotes::extract_remote_names(&body) {
                for name in names {
                    map.entry(name).or_default();
                }
            }
        }
    }

    if let Ok(mounts) = crate::mounts::load_from_dir(config_dir) {
        let active = match app.try_state::<RcdState>() {
            Some(state) => crate::mounts::active_mount_points(&state).await.unwrap_or_default(),
            None => Vec::new(),
        };
        for m in mounts {
            if let Some(remote) = remote_name_of(&m.remote) {
                let mounted = crate::mounts::is_mounted(&active, &m.mount_point);
                map.entry(remote.to_string()).or_default().mount = Some((m.name, mounted));
            }
        }
    }

    if let Ok(jobs) = crate::jobs::load_from_dir(config_dir) {
        for j in jobs {
            if let Some(remote) = remote_name_of(&j.source).or_else(|| remote_name_of(&j.destination)) {
                let auto_active = j.auto_interval_minutes.is_some();
                let last_failed = j.history.first().is_some_and(|h| !h.success);
                map.entry(remote.to_string()).or_default().backup = Some(JobStatus { name: j.name, auto_active, last_failed });
            }
        }
    }

    if let Ok(bisync_jobs) = crate::bisync::load_from_dir(config_dir) {
        for j in bisync_jobs {
            if let Some(remote) = remote_name_of(&j.path1).or_else(|| remote_name_of(&j.path2)) {
                let auto_active = j.auto_interval_minutes.is_some();
                let last_failed = j.history.first().is_some_and(|h| !h.success);
                map.entry(remote.to_string()).or_default().bisync = Some(JobStatus { name: j.name, auto_active, last_failed });
            }
        }
    }

    let mut result: Vec<_> = map.into_iter().collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

fn build_remote_submenu(app: &AppHandle, remote_name: &str, actions: &RemoteActions) -> tauri::Result<Submenu<Wry>> {
    let submenu = Submenu::new(app, remote_name, true)?;

    // Remote senza nulla di configurato: nessuna azione rapida da offrire,
    // solo un rimando alla home per impostarne una.
    if actions.is_empty() {
        submenu.append(&MenuItem::with_id(app, format!("{CONFIGURE_PREFIX}{remote_name}"), "Configura", true, None::<&str>)?)?;
        return Ok(submenu);
    }

    // "Monta" è disabilitato (ma resta visibile, così l'utente capisce
    // perché) se un backup o una bisync di questo remote ha la
    // sincronizzazione automatica accesa — stesso identico criterio che
    // `mounts::mount_now` verifica davvero lato backend
    // (`activity::ensure_no_other_active_service`), altrimenti il tasto
    // risulterebbe cliccabile ma il montaggio fallirebbe silenziosamente
    // (l'unico avviso sarebbe una notifica desktop). "Smonta" non ha questo
    // vincolo: è sempre disponibile quando il remote è montato.
    let remote_job_active = actions.backup.as_ref().is_some_and(|j| j.auto_active) || actions.bisync.as_ref().is_some_and(|j| j.auto_active);

    if let Some((mount_name, mounted)) = &actions.mount {
        let (label, id, enabled) = if *mounted {
            ("Smonta".to_string(), format!("{UNMOUNT_PREFIX}{mount_name}"), true)
        } else {
            ("Monta e apri".to_string(), format!("{MOUNT_PREFIX}{mount_name}"), !remote_job_active)
        };
        submenu.append(&MenuItem::with_id(app, id, label, enabled, None::<&str>)?)?;
    }
    if let Some(job) = &actions.backup {
        submenu.append(&MenuItem::with_id(app, format!("{BACKUP_PREFIX}{}", job.name), "Backup ora", true, None::<&str>)?)?;
    }
    if let Some(job) = &actions.bisync {
        submenu.append(&MenuItem::with_id(app, format!("{BISYNC_PREFIX}{}", job.name), "Sincronizza ora", true, None::<&str>)?)?;
    }

    Ok(submenu)
}

/// Ricostruisce il menu con Mostra/Nascondi, un sottomenu per ogni remote
/// con almeno un mount/backup/bisync configurato, ed Esci — chiamata
/// periodicamente da `watch_menu` così riflette configurazioni cambiate
/// mentre l'app è in esecuzione (nuovo remote/job aggiunto o rimosso, mount
/// montato/smontato) senza dover agganciare un evento esplicito ad ogni
/// singolo comando che tocca mount/job/bisync.
///
/// Raccoglie prima TUTTI i dati (unica parte che ha bisogno di `.await`,
/// per lo stato live dei mount), poi costruisce gli oggetti menu di Tauri
/// in una funzione tutta sincrona: non sono `Send`, non possono
/// attraversare un punto di sospensione dentro il task spawnato da
/// `watch_menu`.
async fn build_dynamic_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let remotes = match app.path().app_config_dir() {
        Ok(config_dir) => collect_remote_actions(app, &config_dir).await,
        Err(_) => Vec::new(),
    };
    build_menu_from_remotes(app, &remotes)
}

/// Etichetta di una voce di avviso in cima al menu per un remote la cui
/// ultima esecuzione registrata è fallita — nomina il tipo di job fallito
/// così l'utente sa cosa aspettarsi aprendo la cronologia, senza doverlo
/// scoprire cliccando.
fn warning_label(remote_name: &str, actions: &RemoteActions) -> Option<String> {
    let backup_failed = actions.backup.as_ref().is_some_and(|j| j.last_failed);
    let bisync_failed = actions.bisync.as_ref().is_some_and(|j| j.last_failed);
    match (backup_failed, bisync_failed) {
        (true, true) => Some(format!("⚠ {remote_name}: backup e sincronizzazione falliti")),
        (true, false) => Some(format!("⚠ {remote_name}: backup fallito")),
        (false, true) => Some(format!("⚠ {remote_name}: sincronizzazione fallita")),
        (false, false) => None,
    }
}

fn build_menu_from_remotes(app: &AppHandle, remotes: &[(String, RemoteActions)]) -> tauri::Result<Menu<Wry>> {
    let show_hide = MenuItem::with_id(app, SHOW_HIDE_ID, "Mostra/Nascondi finestra", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, SETTINGS_ID, "Impostazioni", true, None::<&str>)?;
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = vec![Box::new(show_hide), Box::new(settings)];

    if let Some(version) = pending_update(app) {
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
        items.push(Box::new(MenuItem::with_id(app, UPDATE_ID, format!("⬆ Aggiornamento disponibile: {version}"), true, None::<&str>)?));
    }

    let warnings: Vec<(&str, String)> =
        remotes.iter().filter_map(|(remote_name, actions)| warning_label(remote_name, actions).map(|label| (remote_name.as_str(), label))).collect();
    if !warnings.is_empty() {
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
        for (remote_name, label) in warnings {
            items.push(Box::new(MenuItem::with_id(app, format!("{WARNING_PREFIX}{remote_name}"), label, true, None::<&str>)?));
        }
    }

    if !remotes.is_empty() {
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
        for (remote_name, actions) in remotes {
            items.push(Box::new(build_remote_submenu(app, remote_name, actions)?));
        }
    }

    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(MenuItem::with_id(app, QUIT_ID, "Esci", true, None::<&str>)?));

    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|item| item.as_ref()).collect();
    Menu::with_items(app, &refs)
}

async fn watch_menu(app: AppHandle) {
    loop {
        if let Ok(menu) = build_dynamic_menu(&app).await {
            if let Some(tray) = app.try_state::<TrayHandle>() {
                let _ = tray.0.set_menu(Some(menu));
            }
        }
        tokio::time::sleep(MENU_REFRESH_INTERVAL).await;
    }
}

/// Esegue l'azione codificata nell'id della voce di menu dinamica cliccata
/// (vedi `build_remote_submenu`), in un task separato perché
/// `on_menu_event` non è async e queste azioni (mount/sync reali) possono
/// durare a lungo.
///
/// Il lavoro vero gira in un secondo task annidato (`work` sotto) solo per
/// poterne intercettare un eventuale panic tramite il suo `JoinHandle`: un
/// panic in un task "fire and forget" come questo morirebbe altrimenti in
/// silenzio (visibile solo nello stderr del processo, mai all'utente) — un
/// clic su "Sincronizza ora" dalla tray sembrerebbe non fare nulla. Un
/// fallimento normale (`Err`) o un panic arrivano ora entrambi come la
/// stessa notifica desktop, l'unica UI raggiungibile da qui.
fn dispatch_action(app: &AppHandle, id: &str) {
    let Some((prefix, name)) = id.split_once(':') else { return };
    let prefix = format!("{prefix}:");
    let name = name.to_string();
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        let work = {
            let app = app.clone();
            let name = name.clone();
            let prefix = prefix.clone();
            tauri::async_runtime::spawn(async move {
                match prefix.as_str() {
                    MOUNT_PREFIX => {
                        let state = app.state::<RcdState>();
                        crate::mounts::mount_now_and_open(app.clone(), state, name.clone()).await
                    }
                    UNMOUNT_PREFIX => {
                        let state = app.state::<RcdState>();
                        crate::mounts::unmount_now(app.clone(), state, name.clone()).await
                    }
                    BACKUP_PREFIX => {
                        let state = app.state::<RcdState>();
                        crate::jobs::run_job(app.clone(), state, name.clone()).await
                    }
                    BISYNC_PREFIX => {
                        let state = app.state::<RcdState>();
                        // A differenza di jobs::run_job, un fallimento
                        // riportato da rclone stesso (password sbagliata,
                        // conflitti irrisolvibili, ecc.) non arriva come
                        // `Err` ma come `Ok(BisyncRunResult { success:
                        // false, .. })` — va tradotto qui in `Err`, altrimenti
                        // il ramo sotto lo tratterebbe come un successo e
                        // mostrerebbe "completato" anche per un run fallito.
                        match crate::bisync::run_bisync_job(app.clone(), state, name.clone()).await {
                            Ok(result) if result.success => Ok(()),
                            Ok(result) => Err(result.message),
                            Err(e) => Err(e),
                        }
                    }
                    _ => Ok(()),
                }
            })
        };

        match work.await {
            // Ogni azione lanciata dalla tray (Monta/Smonta, Backup ora,
            // Sincronizza ora) deve dare un riscontro con l'esito, non solo
            // mount (che in più apre la cartella, ma senza notifica
            // l'utente resterebbe comunque incerto se il click fosse partito).
            Ok(Ok(())) => notify_done(&app, &name, &prefix),
            Ok(Err(message)) => notify_error(&app, &name, &message),
            Err(_) => notify_error(&app, &name, "l'operazione si è interrotta inaspettatamente, riprova"),
        }
    });
}

/// Entrambe le notifiche sotto girano su un thread OS a parte, non nel task
/// tokio chiamante: chiamare l'API di notifica da dentro un task già in
/// esecuzione sulla runtime manda in panic con "Cannot start a runtime from
/// within a runtime" — il plugin apre una propria runtime per il D-Bus di
/// sistema, cosa che Tokio non permette da un suo stesso worker thread. Un
/// panic qui terminerebbe il task silenziosamente, senza che la notifica
/// appaia mai.
fn notify_error(app: &AppHandle, name: &str, message: &str) {
    let app = app.clone();
    let name = name.to_string();
    let message = message.to_string();
    std::thread::spawn(move || {
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().builder().title("Rclone Easy").body(format!("'{name}' non è riuscito: {message}")).show();
    });
}

fn notify_done(app: &AppHandle, name: &str, prefix: &str) {
    let app = app.clone();
    let body = match prefix {
        MOUNT_PREFIX => format!("'{name}' montato."),
        UNMOUNT_PREFIX => format!("'{name}' smontato."),
        _ => format!("'{name}' completato."),
    };
    std::thread::spawn(move || {
        use tauri_plugin_notification::NotificationExt;
        let _ = app.notification().builder().title("Rclone Easy").body(body).show();
    });
}

/// Non usa `app.exit(0)`: quello passa dal normale ciclo eventi di Tauri/
/// tao, che sul bug Wayland/KDE con le decorazioni CSD (vedi il commento su
/// `tauri.conf.json::decorations`) può restare bloccato se la finestra è
/// nello stato "decorazioni non responsive" (osservato: "Esci" dal tray in
/// quello stato lasciava l'app in uno stato inconsistente, serviva killarla
/// a mano). Spegnere il demone e terminare direttamente il processo —
/// stesso schema già usato per SIGTERM in `spawn_signal_shutdown_handler`
/// (`lib.rs`) — garantisce l'uscita anche se il loop eventi della finestra
/// è bloccato.
fn perform_quit(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Vedi `spawn_signal_shutdown_handler` (lib.rs) per il perché: senza
        // questo, un bisync in corso resterebbe orfano in background con il
        // suo lock file mai rilasciato.
        crate::bisync::terminate_running_bisyncs();
        let state = app.state::<RcdState>();
        crate::rcd::shutdown(&state).await;
        std::process::exit(0);
    });
}

/// Nasconde la finestra dall'interfaccia stessa — da quando la finestra non
/// ha più una barra del titolo nativa (bug Wayland/KDE aggirato eliminando
/// del tutto le decorazioni CSD, vedi `tauri.conf.json`), la X non esiste
/// più: questo bottone in `+layout.svelte` la sostituisce con lo stesso
/// comportamento (nasconde in tray, l'app resta attiva) di `SHOW_HIDE_ID`.
/// Niente equivalente in-app per l'uscita vera (era un secondo comando
/// `quit_app`, tolto perché due pulsanti confondevano l'utente) — resta
/// raggiungibile solo da "Esci" nel menu della tray (`QUIT_ID` sopra,
/// tramite `perform_quit`).
#[tauri::command]
pub fn hide_window(app: AppHandle) {
    hide_main_window(&app);
}

/// Mostra direttamente il modal di aggiornamento se c'è un aggiornamento in
/// sospeso quando la finestra passa da nascosta a visibile — richiesta
/// esplicita di Simone (19/8/2026): non farlo scoprire solo aprendo
/// l'app "a caso", visto che l'icona ambra nella tray già lo segnala.
/// Nessun effetto se la finestra era già visibile (viene solo nascosta).
fn toggle_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else { return };
    if window.is_visible().unwrap_or(false) {
        hide_main_window(app);
    } else {
        show_main_window(app);
        if pending_update(app).is_some() {
            let _ = app.emit(OPEN_UPDATE_EVENT, ());
        }
    }
}

pub(crate) fn show_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else { return };
    let _ = window.show();
    let _ = window.set_focus();
}

/// Nasconde la finestra principale. Punto unico usato sia dal toggle della
/// tray sia dall'intercettazione della chiusura
/// (`lib.rs::hide_instead_of_close`).
pub(crate) fn hide_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else { return };
    let _ = window.hide();
}

/// Porta la finestra in primo piano e notifica il frontend (`+layout.svelte`,
/// in ascolto su `FOCUS_REMOTE_EVENT`) di navigare alla pagina del remote
/// indicato, aprendola sulla cronologia se si veniva da una voce di avviso —
/// vedi `on_menu_event` per "Configura" e le voci di avviso.
fn focus_remote(app: &AppHandle, remote: &str, open_history: bool) {
    show_main_window(app);
    let _ = app.emit(FOCUS_REMOTE_EVENT, serde_json::json!({ "remote": remote, "openHistory": open_history }));
}

/// Porta la finestra in primo piano e chiede al frontend (`SettingsButton.svelte`,
/// in ascolto su `OPEN_SETTINGS_EVENT`) di aprire il modal Impostazioni —
/// voce "Impostazioni" del menu della tray.
fn open_settings(app: &AppHandle) {
    show_main_window(app);
    let _ = app.emit(OPEN_SETTINGS_EVENT, ());
}

/// Porta la finestra in primo piano e chiede al frontend (`UpdateButton.svelte`,
/// in ascolto su `OPEN_UPDATE_EVENT`) di aprire direttamente il modal di
/// aggiornamento — voce "⬆ Aggiornamento disponibile" del menu della tray.
fn open_update(app: &AppHandle) {
    show_main_window(app);
    let _ = app.emit(OPEN_UPDATE_EVENT, ());
}
