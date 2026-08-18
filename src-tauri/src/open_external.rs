/// Variabili d'ambiente che l'`AppRun` generato da `linuxdeploy` (bundling
/// AppImage) inietta puntando alle librerie/moduli bundlati nell'AppImage
/// invece che a quelli di sistema — causano il fallimento silenzioso di
/// `xdg-open` lanciato da dentro l'AppImage (spawn riuscito, exit 0, nessuna
/// finestra/browser). L'AppImage bundla una propria copia di `xdg-open`
/// identica byte per byte a quella di sistema, eppure invocata con questo
/// ambiente non funziona — causa non identificata con certezza, ma il
/// sintomo sparisce forzando la risoluzione verso i binari/librerie di
/// sistema. Riusata sia per aprire percorsi/URL da questo processo sia per
/// ripulire l'ambiente del sottoprocesso `rclone rcd` (vedi `rcd.rs`), il
/// cui `xdg-open` interno per l'apertura automatica del browser OAuth
/// soffre dello stesso problema essendo un suo figlio diretto.
#[cfg(target_os = "linux")]
pub(crate) const APPIMAGE_POLLUTED_ENV_VARS: &[&str] = &[
    "LD_LIBRARY_PATH",
    "XDG_DATA_DIRS",
    "GSETTINGS_SCHEMA_DIR",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "QT_PLUGIN_PATH",
    "PERLLIB",
    "PYTHONPATH",
    "GIO_MODULE_DIR",
    "GIO_EXTRA_MODULES",
];

/// `PATH` di sistema "pulito", senza il `$APPDIR/usr/bin` che l'AppImage
/// antepone e dove bundla una propria copia di `xdg-open` — quella copia,
/// pur identica byte per byte a quella di sistema, non funziona da lì (vedi
/// sopra). Riusato anche per il sottoprocesso `rclone rcd` (`rcd.rs`): il
/// suo `xdg-open` interno per l'apertura del browser OAuth farebbe la stessa
/// risoluzione via `PATH` e troverebbe la stessa copia rotta se ereditasse
/// il `PATH` dell'AppImage.
#[cfg(target_os = "linux")]
pub(crate) const CLEAN_SYSTEM_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

#[cfg(target_os = "linux")]
fn xdg_open(arg: &str) {
    let mut command = std::process::Command::new("xdg-open");
    command.arg(arg);
    for var in APPIMAGE_POLLUTED_ENV_VARS {
        command.env_remove(var);
    }
    command.env("PATH", CLEAN_SYSTEM_PATH);
    let _ = command.spawn();
}

/// Apre `path` nel file manager di sistema. Fuori da Linux non c'è un
/// runtime AppImage a inquinare l'ambiente, `tauri_plugin_opener` basta.
pub(crate) fn open_path(path: &str) {
    #[cfg(target_os = "linux")]
    xdg_open(path);
    #[cfg(not(target_os = "linux"))]
    let _ = tauri_plugin_opener::open_path(path, None::<&str>);
}

/// Apre `url` nel browser di sistema — stesso fix di `open_path`, usata sia
/// per i link "apri manualmente" dell'app (About, autorizzazione OAuth) sia
/// come comando esposto al frontend al posto della funzione JS del plugin,
/// che nell'AppImage soffrirebbe dello stesso fallimento silenzioso.
pub(crate) fn open_url(url: &str) {
    #[cfg(target_os = "linux")]
    xdg_open(url);
    #[cfg(not(target_os = "linux"))]
    let _ = tauri_plugin_opener::open_url(url, None::<&str>);
}

#[tauri::command]
pub fn open_url_in_browser(url: String) {
    open_url(&url);
}
