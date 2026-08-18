use crate::rcd::{self, RcdState};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// Mount configurato (`rclone rc mount/mount`) — un concetto proprio di
/// Rclone Easy, non di rclone stesso, stesso schema di `SyncJob` in
/// `jobs.rs`. `remote` è già nel formato che `mount/mount` si aspetta
/// (`remoto:percorso`), costruito dal frontend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MountConfig {
    pub name: String,
    pub remote: String,
    pub mount_point: String,
    /// Se `true`, montato automaticamente all'avvio dell'app (`lib.rs`,
    /// dopo l'avvio del demone rclone rcd) — best-effort, un fallimento non
    /// impedisce l'avvio dell'app né blocca gli altri mount automatici.
    #[serde(default)]
    pub auto_mount: bool,
    /// Gli ultimi tentativi di monta/smonta, più recente in testa — a
    /// differenza dei job di sync/bisync, prima di questo il mount non
    /// teneva traccia di nulla (`mount_now`/`unmount_now` propagavano solo
    /// l'errore al chiamante senza persisterlo), quindi non c'era modo di
    /// consultare un tentativo passato dopo il fatto.
    #[serde(default)]
    pub history: Vec<MountEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MountEvent {
    pub action: String,
    pub success: bool,
    pub message: String,
    pub when_unix: u64,
}

/// Config unita allo stato live (`mounted`): a differenza dei job di sync,
/// un mount ha uno stato "in corso" che ha senso mostrare in dashboard, e
/// che rclone stesso conosce meglio di noi (`mount/listmounts`) — non lo
/// persistiamo mai, perché non sopravvive comunque al riavvio dell'app
/// (`rclone rcd` riparte "vuoto" ogni volta, stessa scelta già presa per i
/// job: solo esecuzione manuale, nessun remount automatico allo startup).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MountEntry {
    pub name: String,
    pub remote: String,
    pub mount_point: String,
    pub mounted: bool,
    pub auto_mount: bool,
    pub history: Vec<MountEvent>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MountsFile {
    #[serde(default)]
    mounts: Vec<MountConfig>,
}

fn mounts_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("mounts.toml")
}

pub(crate) fn load_from_dir(config_dir: &Path) -> Result<Vec<MountConfig>, String> {
    let path = mounts_file_path(config_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("impossibile leggere '{}': {e}", path.display()))?;
    let file: MountsFile = toml::from_str(&content).map_err(|e| format!("file dei mount non valido: {e}"))?;
    Ok(file.mounts)
}

fn save_to_dir(config_dir: &Path, mounts: &[MountConfig]) -> Result<(), String> {
    std::fs::create_dir_all(config_dir)
        .map_err(|e| format!("impossibile creare '{}': {e}", config_dir.display()))?;
    let file = MountsFile { mounts: mounts.to_vec() };
    let content = toml::to_string_pretty(&file).map_err(|e| format!("impossibile serializzare i mount: {e}"))?;
    crate::fs_atomic::write_atomically(&mounts_file_path(config_dir), &content)
}

/// Riscrive il campo `remote` di ogni mount che referenzia `old_name` per
/// puntare a `new_name`, lasciando invariato il resto del percorso dopo i
/// due punti — usata solo da `remotes::rename_remote_in`: rclone non ha un
/// `config/rename` via RC (investigato: non esiste), rinominare un remote
/// è quindi crea con lo stesso nome sotto quello nuovo + aggiorna ogni
/// riferimento + cancella il vecchio, vedi lì per l'ordine e il perché.
pub(crate) fn rename_remote_references_in(config_dir: &Path, old_name: &str, new_name: &str) -> Result<(), String> {
    let mut mounts = load_from_dir(config_dir)?;
    let mut changed = false;
    for m in mounts.iter_mut() {
        if remote_name_of(&m.remote) == Some(old_name) {
            m.remote = format!("{new_name}:{}", m.remote.split_once(':').map(|(_, rest)| rest).unwrap_or(""));
            changed = true;
        }
    }
    if changed {
        save_to_dir(config_dir, &mounts)?;
    }
    Ok(())
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

/// Rifiuta un secondo mount per lo stesso remote: al massimo un mount per
/// remote, per tenere l'interfaccia semplice (un solo posto dove trovare
/// "il mount di X" invece di doverne gestire più d'uno). `excluding` è il
/// nome della voce in corso di modifica, da non contare come "già esistente".
fn ensure_remote_not_already_mounted(mounts: &[MountConfig], remote: &str, excluding: Option<&str>) -> Result<(), String> {
    let Some(remote_name) = remote_name_of(remote) else { return Ok(()) };
    let clash = mounts
        .iter()
        .filter(|m| excluding != Some(m.name.as_str()))
        .any(|m| remote_name_of(&m.remote) == Some(remote_name));
    if clash {
        return Err(format!("esiste già un mount per '{remote_name}': modificalo invece di crearne un altro"));
    }
    Ok(())
}

pub(crate) fn create_mount_in(config_dir: &Path, name: &str, remote: &str, mount_point: &str, auto_mount: bool) -> Result<(), String> {
    let mut mounts = load_from_dir(config_dir)?;
    if mounts.iter().any(|m| m.name == name) {
        return Err(format!("esiste già un mount chiamato '{name}': scegli un altro nome"));
    }
    ensure_remote_not_already_mounted(&mounts, remote, None)?;
    mounts.push(MountConfig {
        name: name.to_string(),
        remote: remote.to_string(),
        mount_point: mount_point.to_string(),
        auto_mount,
        history: Vec::new(),
    });
    save_to_dir(config_dir, &mounts)
}

/// `old_name` individua la voce da modificare, `name` è il nome (nuovo o
/// invariato) dopo la modifica — permette di rinominare un mount insieme
/// alle altre modifiche in un solo salvataggio.
fn update_mount_in(
    config_dir: &Path,
    old_name: &str,
    name: &str,
    remote: &str,
    mount_point: &str,
    auto_mount: bool,
) -> Result<(), String> {
    let mut mounts = load_from_dir(config_dir)?;
    if name != old_name && mounts.iter().any(|m| m.name == name) {
        return Err(format!("esiste già un mount chiamato '{name}': scegli un altro nome"));
    }
    ensure_remote_not_already_mounted(&mounts, remote, Some(old_name))?;
    let mount = mounts.iter_mut().find(|m| m.name == old_name).ok_or_else(|| format!("nessun mount chiamato '{old_name}'"))?;
    mount.name = name.to_string();
    mount.remote = remote.to_string();
    mount.mount_point = mount_point.to_string();
    mount.auto_mount = auto_mount;
    save_to_dir(config_dir, &mounts)
}

pub(crate) fn delete_mount_in(config_dir: &Path, name: &str) -> Result<(), String> {
    let mut mounts = load_from_dir(config_dir)?;
    let before = mounts.len();
    mounts.retain(|m| m.name != name);
    if mounts.len() == before {
        return Err(format!("nessun mount chiamato '{name}'"));
    }
    save_to_dir(config_dir, &mounts)
}

/// Percorsi dei mount attualmente attivi nel demone (`mount/listmounts`) —
/// unica fonte di verità per "montato o no", vedi `MountEntry` sopra.
pub(crate) async fn active_mount_points(state: &RcdState) -> Result<Vec<String>, String> {
    let body = rcd::call(state, "mount/listmounts", serde_json::json!({})).await?;
    Ok(body
        .get("mountPoints")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().filter_map(|entry| entry.get("MountPoint").and_then(|v| v.as_str()).map(str::to_string)).collect()
        })
        .unwrap_or_default())
}

pub(crate) fn is_mounted(active: &[String], mount_point: &str) -> bool {
    active.iter().any(|p| Path::new(p) == Path::new(mount_point))
}

/// `true` per un percorso a lettera di unità Windows (`X:` o `X:\`) — non è
/// una cartella reale sul filesystem, non va mai toccato da
/// `prepare_windows_mount_point` sotto. Separata dal resto (non gated a
/// `cfg(windows)`) solo per poterla testare anche su questa piattaforma di
/// sviluppo, stesso schema di `rcd::is_our_orphaned_rcd`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_drive_letter_mount_point(path: &str) -> bool {
    let chars: Vec<char> = path.chars().collect();
    matches!(chars.as_slice(), [letter, ':'] | [letter, ':', '\\'] if letter.is_ascii_alphabetic())
}

/// `true` per i metadati che Windows/Explorer scrivono da soli in una
/// cartella altrimenti vuota (`desktop.ini`, personalizzazioni della
/// cartella; `Thumbs.db`, cache miniature su filesystem senza indicizzazione
/// abilitata) — mai dati dell'utente, quindi non devono impedire di
/// considerare "vuota" una cartella di mount che all'utente appare tale in
/// Esplora Risorse. Confronto case-insensitive: Windows non distingue
/// maiuscole/minuscole nei nomi file. Non gated a `cfg(windows)` solo per
/// poterla testare anche su questa piattaforma di sviluppo.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_ignorable_windows_metadata_file(file_name: &str) -> bool {
    matches!(file_name.to_lowercase().as_str(), "desktop.ini" | "thumbs.db")
}

/// Su Windows, WinFsp (usato da rclone per il mount) rifiuta di montare su
/// una cartella che esiste già — `"mountpoint path already exists"` — il
/// contrario esatto di Linux/macOS (FUSE), dove la cartella DEVE già
/// esistere. Il selettore di cartelle della UI fa scegliere solo cartelle
/// esistenti (comportamento corretto per Linux/macOS): su Windows va quindi
/// rimossa appena prima di montare, sicuro solo se risulta vuota — a parte
/// eventuali metadati di Explorer (sopra), mai dati reali dell'utente — perché
/// WinFsp la ricrea lui stesso. Se contiene altro, meglio un errore chiaro
/// qui che lasciare a rclone il messaggio criptico originale.
#[cfg(target_os = "windows")]
fn prepare_windows_mount_point(mount_point: &str) -> Result<(), String> {
    let path = Path::new(mount_point);
    if is_drive_letter_mount_point(mount_point) || !path.exists() {
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!("'{mount_point}' esiste già e non è una cartella"));
    }
    let entries: Vec<_> = std::fs::read_dir(path)
        .map_err(|e| format!("impossibile leggere '{mount_point}': {e}"))?
        .filter_map(Result::ok)
        .collect();
    let has_real_content =
        entries.iter().any(|entry| !is_ignorable_windows_metadata_file(&entry.file_name().to_string_lossy()));
    if has_real_content {
        return Err(format!(
            "'{mount_point}' esiste già e non è vuota: su Windows la cartella di mount non deve esistere ancora, scegline una vuota o non ancora creata"
        ));
    }
    for entry in &entries {
        std::fs::remove_file(entry.path())
            .map_err(|e| format!("impossibile preparare '{mount_point}' per il mount: {e}"))?;
    }
    std::fs::remove_dir(path).map_err(|e| format!("impossibile preparare '{mount_point}' per il mount: {e}"))
}

#[cfg(not(target_os = "windows"))]
fn prepare_windows_mount_point(_mount_point: &str) -> Result<(), String> {
    Ok(())
}

fn app_config_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))
}

#[tauri::command]
pub async fn list_mounts(app: AppHandle, state: tauri::State<'_, RcdState>) -> Result<Vec<MountEntry>, String> {
    let configs = load_from_dir(&app_config_dir(&app)?)?;
    // Se l'interrogazione dello stato live fallisce (es. rclone rcd non
    // avviato) l'elenco resta comunque consultabile, solo con "mounted"
    // sempre false invece di far fallire l'intera dashboard.
    let active = active_mount_points(&state).await.unwrap_or_default();
    Ok(configs
        .into_iter()
        .map(|c| {
            let mounted = is_mounted(&active, &c.mount_point);
            MountEntry {
                name: c.name,
                remote: c.remote,
                mount_point: c.mount_point,
                mounted,
                auto_mount: c.auto_mount,
                history: c.history,
            }
        })
        .collect())
}

/// Se `auto_mount` è impostato (il mount diventa "attivo" già da ora, vedi
/// `activity.rs`), rifiuta se un altro servizio è già attivo per il
/// remote — un mount solo manuale non ha questa restrizione, resta sempre
/// possibile crearne/modificarne uno.
async fn ensure_activation_allowed(state: &RcdState, config_dir: &Path, remote: &str, auto_mount: bool) -> Result<(), String> {
    if !auto_mount {
        return Ok(());
    }
    if let Some(remote_name) = remote_name_of(remote) {
        crate::activity::ensure_no_other_active_service(state, config_dir, remote_name, crate::activity::ServiceKind::Mount)
            .await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn create_mount(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    name: String,
    remote: String,
    mount_point: String,
    auto_mount: bool,
) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    ensure_activation_allowed(&state, &config_dir, &remote, auto_mount).await?;
    create_mount_in(&config_dir, &name, &remote, &mount_point, auto_mount)
}

/// Rifiuta modifica/eliminazione se il mount indicato risulta attualmente
/// attivo: cambiare o rimuovere la voce persistita mentre è montato
/// lascerebbe la config disallineata da quanto rclone ha davvero in corso
/// (il mount live continuerebbe con i vecchi parametri, ignaro della
/// modifica).
async fn refuse_if_mounted(state: &RcdState, mount: &MountConfig, action: &str) -> Result<(), String> {
    let active = active_mount_points(state).await.unwrap_or_default();
    if is_mounted(&active, &mount.mount_point) {
        return Err(format!("smonta prima di {action} questo mount"));
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_mount(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    let mounts = load_from_dir(&config_dir)?;
    let mount = mounts.iter().find(|m| m.name == name).ok_or_else(|| format!("nessun mount chiamato '{name}'"))?;
    refuse_if_mounted(&state, mount, "eliminare").await?;
    delete_mount_in(&config_dir, &name)
}

#[tauri::command]
pub async fn update_mount(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    old_name: String,
    name: String,
    remote: String,
    mount_point: String,
    auto_mount: bool,
) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    let mounts = load_from_dir(&config_dir)?;
    let mount = mounts.iter().find(|m| m.name == old_name).ok_or_else(|| format!("nessun mount chiamato '{old_name}'"))?;
    refuse_if_mounted(&state, mount, "modificare").await?;
    ensure_activation_allowed(&state, &config_dir, &remote, auto_mount).await?;
    update_mount_in(&config_dir, &old_name, &name, &remote, &mount_point, auto_mount)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Registra l'esito di un tentativo di monta/smonta nello storico del
/// mount — best-effort (un errore qui, es. impossibile scrivere su disco,
/// non deve mascherare l'esito vero del tentativo, riportato comunque dal
/// chiamante indipendentemente da questa funzione).
fn record_mount_event(config_dir: &Path, name: &str, action: &str, result: &Result<(), String>) {
    let Ok(mut mounts) = load_from_dir(config_dir) else { return };
    let Some(stored) = mounts.iter_mut().find(|m| m.name == name) else { return };
    let event = match result {
        Ok(()) => MountEvent { action: action.to_string(), success: true, message: String::new(), when_unix: now_unix() },
        Err(e) => MountEvent { action: action.to_string(), success: false, message: e.clone(), when_unix: now_unix() },
    };
    crate::jobs::push_history(&mut stored.history, event);
    let _ = save_to_dir(config_dir, &mounts);
}

/// Su Windows, montare senza WinFsp installato fallisce con un messaggio di
/// rclone tecnico e in inglese (`cgofuse: cannot find winfsp`, con un link
/// all'installer) — WinFsp non è bundlato né rilevato in anticipo dall'app
/// (richiede un installer MSI a parte, fuori dal nostro controllo), quindi
/// l'errore va e viene esattamente quando l'utente ci sbatte contro per la
/// prima volta. Qui lo si riconosce e si sostituisce con un messaggio
/// comprensibile nella stessa lingua del resto dell'app, con l'indicazione
/// diretta di dove scaricarlo — altri errori di mount passano invariati.
fn friendly_mount_error(raw: String) -> String {
    if raw.to_lowercase().contains("cannot find winfsp") {
        "Per montare un remote su Windows serve WinFsp, non incluso in Rclone Easy. Usa il pulsante \"Installa WinFsp\" qui sotto, poi riprova a montare.".to_string()
    } else {
        raw
    }
}

/// Estrae l'URL di download dell'ultimo installer `.msi` di WinFsp dalla
/// risposta JSON di GitHub (`GET .../releases/latest`) — separata dalla
/// chiamata di rete per poterla testare con dati sintetici.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn find_msi_download_url(release: &serde_json::Value) -> Result<String, String> {
    release
        .get("assets")
        .and_then(|assets| assets.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                let name = asset.get("name")?.as_str()?;
                if !name.ends_with(".msi") {
                    return None;
                }
                asset.get("browser_download_url")?.as_str().map(str::to_string)
            })
        })
        .ok_or_else(|| "nessun installer .msi trovato nell'ultima release di WinFsp su GitHub".to_string())
}

/// Scarica ed esegue l'installer ufficiale di WinFsp, invece di ridistribuirlo
/// dentro il pacchetto di Rclone Easy: la licenza di WinFsp (GPLv3 con
/// un'eccezione FLOSS, più una licenza commerciale a pagamento) non è stata
/// verificata da noi per la ridistribuzione, mentre scaricarlo al momento dal
/// repository ufficiale non solleva questo dubbio. Interroga la API "ultima
/// release" di GitHub invece di un URL fisso, perché il nome del file `.msi`
/// include il numero di versione e cambia a ogni release. L'installer viene
/// lanciato in modo interattivo (non silenzioso, niente flag `/quiet`): mostra
/// la propria interfaccia e i propri termini di licenza, lasciando all'utente
/// la scelta di accettarli — evita anche a noi di dover decidere se
/// un'installazione silenziosa sarebbe lecita senza un consenso esplicito.
/// Il comando torna appena l'installer è stato avviato, senza aspettare che
/// l'utente lo completi: la UI deve invitarlo a riprovare a montare dopo.
/// Registrato per tutte le piattaforme (serve un unico elenco di comandi in
/// `lib.rs`), ma ha senso solo su Windows: altrove fallisce subito senza
/// toccare la rete né lanciare nulla.
#[tauri::command]
pub async fn download_and_launch_winfsp_installer() -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    {
        return Err("WinFsp serve solo su Windows: questa piattaforma non ne ha bisogno".to_string());
    }

    #[cfg(target_os = "windows")]
    download_and_launch_winfsp_installer_windows().await
}

#[cfg(target_os = "windows")]
async fn download_and_launch_winfsp_installer_windows() -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("rclone-easy")
        .build()
        .map_err(|e| format!("impossibile preparare la richiesta a GitHub: {e}"))?;

    let release: serde_json::Value = client
        .get("https://api.github.com/repos/winfsp/winfsp/releases/latest")
        .send()
        .await
        .map_err(|e| format!("impossibile contattare GitHub per scaricare WinFsp: {e}"))?
        .json()
        .await
        .map_err(|e| format!("risposta di GitHub non interpretabile: {e}"))?;
    let asset_url = find_msi_download_url(&release)?;

    let bytes = client
        .get(&asset_url)
        .send()
        .await
        .map_err(|e| format!("impossibile scaricare l'installer di WinFsp: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("impossibile scaricare l'installer di WinFsp: {e}"))?;

    let installer_path = std::env::temp_dir().join("rclone-easy-winfsp-installer.msi");
    std::fs::write(&installer_path, &bytes)
        .map_err(|e| format!("impossibile salvare l'installer di WinFsp: {e}"))?;

    let mut command = std::process::Command::new("msiexec");
    command.args(["/i", &installer_path.to_string_lossy()]);
    command.spawn().map_err(|e| format!("impossibile avviare l'installer di WinFsp: {e}"))?;
    Ok(())
}

#[tauri::command]
pub async fn mount_now(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    let mounts = load_from_dir(&config_dir)?;
    let mount = mounts.iter().find(|m| m.name == name).ok_or_else(|| format!("nessun mount chiamato '{name}'"))?.clone();
    if let Some(remote_name) = remote_name_of(&mount.remote) {
        crate::activity::ensure_no_other_active_service(&state, &config_dir, remote_name, crate::activity::ServiceKind::Mount)
            .await?;
    }
    let result = match prepare_windows_mount_point(&mount.mount_point) {
        Ok(()) => rcd::call(&state, "mount/mount", serde_json::json!({ "fs": mount.remote, "mountPoint": mount.mount_point }))
            .await
            .map(|_| ())
            .map_err(friendly_mount_error),
        Err(e) => Err(e),
    };
    record_mount_event(&config_dir, &name, "mount", &result);
    result
}

/// Monta e apre subito la cartella nel file manager di sistema — l'unico
/// modo con cui l'utente ha conferma visiva immediata che il montaggio è
/// riuscito davvero (a differenza di backup/bisync non c'è altrove un
/// segnale altrettanto diretto). Usata sia dal comando invocato dal modal
/// di RemoteRow.svelte sia da `tray::dispatch_action` per la voce "Monta e
/// apri" del menu — punto unico così il comportamento resta identico nei
/// due posti. L'apertura è best-effort: un suo fallimento (es. nessun file
/// manager grafico disponibile) non deve far sembrare fallito il montaggio,
/// che a quel punto è già riuscito.
#[tauri::command]
pub async fn mount_now_and_open(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<(), String> {
    mount_now(app.clone(), state, name.clone()).await?;
    let config_dir = app_config_dir(&app)?;
    if let Ok(mounts) = load_from_dir(&config_dir) {
        if let Some(mount) = mounts.iter().find(|m| m.name == name) {
            open_in_file_manager(&mount.mount_point);
        }
    }
    Ok(())
}

/// Apre `path` nel file manager di sistema — vedi `open_external.rs` per il
/// perché serve un fix specifico all'AppImage su Linux.
fn open_in_file_manager(path: &str) {
    crate::open_external::open_path(path);
}

#[tauri::command]
pub async fn unmount_now(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    let mounts = load_from_dir(&config_dir)?;
    let mount = mounts.iter().find(|m| m.name == name).ok_or_else(|| format!("nessun mount chiamato '{name}'"))?.clone();
    let result =
        rcd::call(&state, "mount/unmount", serde_json::json!({ "mountPoint": mount.mount_point })).await.map(|_| ());
    record_mount_event(&config_dir, &name, "unmount", &result);
    result
}

/// Monta automaticamente all'avvio i mount con `auto_mount: true` — best
/// effort, un fallimento su uno (es. cartella non più esistente, remote
/// temporaneamente non raggiungibile) non blocca gli altri né l'avvio
/// dell'app. Chiamata da `lib.rs` dopo l'avvio del demone rclone rcd.
pub(crate) async fn auto_mount_all(state: &RcdState, config_dir: &Path) {
    let Ok(mounts) = load_from_dir(config_dir) else { return };
    for mount in mounts.iter().filter(|m| m.auto_mount) {
        if prepare_windows_mount_point(&mount.mount_point).is_err() {
            continue;
        }
        let _ =
            rcd::call(state, "mount/mount", serde_json::json!({ "fs": mount.remote, "mountPoint": mount.mount_point }))
                .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;

    #[test]
    fn load_from_missing_file_returns_empty() {
        let dir = TempDir::new("mounts-missing");
        assert_eq!(load_from_dir(&dir.path).unwrap(), Vec::new());
    }

    #[test]
    fn create_then_list_roundtrips() {
        let dir = TempDir::new("mounts-roundtrip");
        create_mount_in(&dir.path, "cloud-lavoro", "cubbit:lavoro", "/home/utente/cloud-lavoro", false).unwrap();

        let mounts = load_from_dir(&dir.path).unwrap();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].name, "cloud-lavoro");
        assert_eq!(mounts[0].remote, "cubbit:lavoro");
        assert_eq!(mounts[0].mount_point, "/home/utente/cloud-lavoro");
    }

    #[test]
    fn create_mount_rejects_duplicate_name() {
        let dir = TempDir::new("mounts-duplicate");
        create_mount_in(&dir.path, "dup", "remote-a:x", "/a", false).unwrap();
        let result = create_mount_in(&dir.path, "dup", "remote-b:y", "/b", false);
        assert!(result.is_err());
        assert_eq!(load_from_dir(&dir.path).unwrap().len(), 1, "il secondo mount non deve essere stato salvato");
    }

    #[test]
    fn create_mount_rejects_a_second_mount_for_the_same_remote() {
        let dir = TempDir::new("mounts-same-remote");
        create_mount_in(&dir.path, "uno", "cubbit:x", "/a", false).unwrap();
        let result = create_mount_in(&dir.path, "due", "cubbit:y", "/b", false);
        assert!(result.is_err());
        assert_eq!(load_from_dir(&dir.path).unwrap().len(), 1, "il secondo mount sullo stesso remote non deve essere stato salvato");
    }

    #[test]
    fn create_mount_allows_different_remotes() {
        let dir = TempDir::new("mounts-different-remotes");
        create_mount_in(&dir.path, "uno", "cubbit:x", "/a", false).unwrap();
        assert!(create_mount_in(&dir.path, "due", "wasabi:y", "/b", false).is_ok());
    }

    #[test]
    fn update_mount_can_keep_pointing_at_its_own_remote() {
        let dir = TempDir::new("mounts-update-same-remote-ok");
        create_mount_in(&dir.path, "uno", "cubbit:x", "/a", false).unwrap();
        // Modificare un mount senza cambiare remote non deve scontrarsi con
        // se stesso.
        assert!(update_mount_in(&dir.path, "uno", "uno", "cubbit:nuovo-percorso", "/a", false).is_ok());
    }

    #[test]
    fn update_mount_rejects_moving_onto_a_remote_already_used_by_another_mount() {
        let dir = TempDir::new("mounts-update-remote-clash");
        create_mount_in(&dir.path, "uno", "cubbit:x", "/a", false).unwrap();
        create_mount_in(&dir.path, "due", "wasabi:y", "/b", false).unwrap();
        let result = update_mount_in(&dir.path, "due", "due", "cubbit:altro-percorso", "/b", false);
        assert!(result.is_err());
    }

    #[test]
    fn update_mount_changes_fields_and_can_rename() {
        let dir = TempDir::new("mounts-update");
        create_mount_in(&dir.path, "vecchio-nome", "remote-a:x", "/vecchio", false).unwrap();

        update_mount_in(&dir.path, "vecchio-nome", "nuovo-nome", "remote-b:y", "/nuovo", true).unwrap();

        let mounts = load_from_dir(&dir.path).unwrap();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].name, "nuovo-nome");
        assert_eq!(mounts[0].remote, "remote-b:y");
        assert_eq!(mounts[0].mount_point, "/nuovo");
    }

    #[test]
    fn update_mount_fails_for_unknown_name() {
        let dir = TempDir::new("mounts-update-unknown");
        assert!(update_mount_in(&dir.path, "non-esiste", "x", "remote:a", "/a", false).is_err());
    }

    #[test]
    fn update_mount_rejects_rename_onto_an_existing_name() {
        let dir = TempDir::new("mounts-update-rename-conflict");
        create_mount_in(&dir.path, "uno", "remote-a:a", "/a", false).unwrap();
        create_mount_in(&dir.path, "due", "remote-b:b", "/b", false).unwrap();

        let result = update_mount_in(&dir.path, "uno", "due", "remote-a:a", "/a", false);
        assert!(result.is_err());
        let mounts = load_from_dir(&dir.path).unwrap();
        assert_eq!(mounts.iter().find(|m| m.name == "uno").unwrap().remote, "remote-a:a", "la voce originale non deve essere stata toccata");
    }

    #[test]
    fn update_mount_keeping_the_same_name_does_not_trigger_the_duplicate_check() {
        let dir = TempDir::new("mounts-update-same-name");
        create_mount_in(&dir.path, "invariato", "remote-a:x", "/a", false).unwrap();

        update_mount_in(&dir.path, "invariato", "invariato", "remote-b:y", "/b", false).unwrap();

        let mounts = load_from_dir(&dir.path).unwrap();
        assert_eq!(mounts[0].remote, "remote-b:y");
    }

    #[test]
    fn delete_mount_removes_it() {
        let dir = TempDir::new("mounts-delete");
        create_mount_in(&dir.path, "da-cancellare", "remote:x", "/a", false).unwrap();
        delete_mount_in(&dir.path, "da-cancellare").unwrap();
        assert_eq!(load_from_dir(&dir.path).unwrap(), Vec::new());
    }

    #[test]
    fn delete_mount_fails_for_unknown_name() {
        let dir = TempDir::new("mounts-delete-unknown");
        assert!(delete_mount_in(&dir.path, "non-esiste").is_err());
    }

    #[test]
    fn friendly_mount_error_translates_the_missing_winfsp_case() {
        let raw = "failed to mount FUSE fs: mount stopped before calling Init: mount failed: cgofuse: cannot find winfsp Hint: Install WinFsp from https://winfsp.dev/rel/".to_string();
        // Il frontend riconosce questo caso specifico cercando "WinFsp" nel
        // messaggio per mostrare il pulsante "Installa WinFsp" — la presenza
        // letterale della parola è quindi parte del contratto, non solo un
        // dettaglio del testo.
        assert!(friendly_mount_error(raw).contains("WinFsp"));
    }

    #[test]
    fn friendly_mount_error_leaves_other_errors_unchanged() {
        let raw = "qualche altro errore di rclone".to_string();
        assert_eq!(friendly_mount_error(raw.clone()), raw);
    }

    #[test]
    fn find_msi_download_url_picks_the_msi_asset_among_others() {
        let release = serde_json::json!({
            "tag_name": "v2.1",
            "assets": [
                { "name": "winfsp-tests-2.1.25156.zip", "browser_download_url": "https://example.com/tests.zip" },
                { "name": "winfsp-2.1.25156.msi", "browser_download_url": "https://example.com/winfsp-2.1.25156.msi" }
            ]
        });
        assert_eq!(find_msi_download_url(&release).unwrap(), "https://example.com/winfsp-2.1.25156.msi");
    }

    #[test]
    fn find_msi_download_url_fails_when_no_msi_asset_is_present() {
        let release = serde_json::json!({ "tag_name": "v2.1", "assets": [] });
        assert!(find_msi_download_url(&release).is_err());
    }

    #[test]
    fn is_ignorable_windows_metadata_file_matches_known_names_case_insensitively() {
        assert!(is_ignorable_windows_metadata_file("desktop.ini"));
        assert!(is_ignorable_windows_metadata_file("Desktop.ini"));
        assert!(is_ignorable_windows_metadata_file("Thumbs.db"));
        assert!(!is_ignorable_windows_metadata_file("documento.txt"));
    }

    #[test]
    fn is_drive_letter_mount_point_matches_bare_and_trailing_backslash_forms() {
        assert!(is_drive_letter_mount_point("X:"));
        assert!(is_drive_letter_mount_point("x:\\"));
        assert!(!is_drive_letter_mount_point("X:\\cartella"));
        assert!(!is_drive_letter_mount_point("/home/utente/cloud"));
        assert!(!is_drive_letter_mount_point("1:"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn prepare_windows_mount_point_is_a_no_op_on_this_platform() {
        // Su Linux/macOS il mountpoint deve continuare a esistere: qui la
        // funzione non deve mai toccare il filesystem, a differenza della
        // variante Windows (non compilata su questa piattaforma).
        let dir = TempDir::new("mounts-windows-prepare-noop");
        std::fs::create_dir_all(&dir.path).unwrap();
        assert!(prepare_windows_mount_point(&dir.path.to_string_lossy()).is_ok());
        assert!(dir.path.exists(), "su questa piattaforma la cartella non deve essere toccata");
    }

    #[test]
    fn is_mounted_compares_by_path_not_by_raw_string() {
        let active = vec!["/home/utente/cloud".to_string()];
        assert!(is_mounted(&active, "/home/utente/cloud"));
        assert!(!is_mounted(&active, "/home/utente/altro"));
    }

    #[test]
    fn record_mount_event_appends_to_the_named_mount_history() {
        let dir = TempDir::new("mounts-record-event");
        create_mount_in(&dir.path, "prova", "remote:x", "/a", false).unwrap();

        record_mount_event(&dir.path, "prova", "mount", &Ok(()));
        record_mount_event(&dir.path, "prova", "unmount", &Err("smontaggio fallito".to_string()));

        let mounts = load_from_dir(&dir.path).unwrap();
        let history = &mounts[0].history;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].action, "unmount", "il più recente deve stare in testa");
        assert!(!history[0].success);
        assert_eq!(history[0].message, "smontaggio fallito");
        assert_eq!(history[1].action, "mount");
        assert!(history[1].success);
    }

    #[test]
    fn record_mount_event_is_a_silent_no_op_for_an_unknown_name() {
        let dir = TempDir::new("mounts-record-event-unknown");
        // Non deve fallire né panicare: nessun mount con quel nome esiste.
        record_mount_event(&dir.path, "non-esiste", "mount", &Ok(()));
    }

    /// Prova un mount/unmount reale via RC API su cartelle temporanee reali
    /// (nessuna credenziale cloud necessaria, come già fatto per la sync in
    /// jobs.rs) — copre sia `mount_now`/`unmount_now` che il calcolo dello
    /// stato "mounted" in `active_mount_points`.
    #[tokio::test]
    async fn mount_then_unmount_a_real_local_folder() {
        let rcd_dir = TempDir::new("mounts-run-rcd");
        let source_dir = TempDir::new("mounts-run-source");
        let mount_dir = TempDir::new("mounts-run-mount");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::create_dir_all(&mount_dir.path).unwrap();
        std::fs::write(source_dir.path.join("prova.txt"), "contenuto di prova").unwrap();

        let state = rcd::build_state(rcd_dir.config_path()).await;
        let mount_point = mount_dir.path.to_string_lossy().to_string();

        rcd::call(
            &state,
            "mount/mount",
            serde_json::json!({ "fs": source_dir.path.to_string_lossy(), "mountPoint": mount_point }),
        )
        .await
        .expect("il mount dovrebbe riuscire");

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(mount_dir.path.join("prova.txt").exists(), "il file della sorgente dovrebbe essere visibile nel mount");

        let active = active_mount_points(&state).await.unwrap();
        assert!(is_mounted(&active, &mount_point), "il mount dovrebbe risultare attivo in mount/listmounts");

        rcd::call(&state, "mount/unmount", serde_json::json!({ "mountPoint": mount_point }))
            .await
            .expect("lo smontaggio dovrebbe riuscire");

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let active_after = active_mount_points(&state).await.unwrap();
        assert!(!is_mounted(&active_after, &mount_point), "il mount non dovrebbe più risultare attivo dopo lo smontaggio");
    }
}
