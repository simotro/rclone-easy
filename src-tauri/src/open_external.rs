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
///
/// `XDG_DATA_DIRS` NON è in questa lista nonostante venga inquinata anche
/// lei (vedi `strip_appimage_data_dirs`): a differenza delle altre, va
/// ripulita solo delle voci che puntano dentro l'AppImage, non rimossa del
/// tutto — conteneva anche percorsi legittimi e necessari (es.
/// `/usr/share/cinnamon` su Cinnamon/Linux Mint, dove risiede
/// l'associazione MIME predefinita per "apri cartella") che una rimozione
/// totale butterebbe via insieme al resto, lasciando `xdg-open` senza modo
/// di trovare il file manager di sistema per quel tipo di apertura
/// specifico — bug reale confermato dal vivo (funzionava per gli URL,
/// mai per le cartelle, e solo su Cinnamon/Mint: coerente, i browser sono
/// quasi sempre associati anche nel fallback generico di glib quando
/// `XDG_DATA_DIRS` manca del tutto, un file manager non è detto che lo sia).
#[cfg(target_os = "linux")]
pub(crate) const APPIMAGE_POLLUTED_ENV_VARS: &[&str] = &[
    "LD_LIBRARY_PATH",
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

/// Toglie da `value` (il contenuto di `XDG_DATA_DIRS`) solo le voci che
/// puntano dentro `mount_dir` (la cartella dove l'AppImage è montata, es.
/// `/tmp/.mount_rcloneXXXXXX`) — mai una rimozione totale, vedi il commento
/// su `APPIMAGE_POLLUTED_ENV_VARS`. Confrontata dopo aver tolto uno
/// eventuale `/` finale: l'AppImage la inserisce sia con sia senza,
/// duplicata (osservato dal vivo: `"$MOUNT/usr/share/:$MOUNT/usr/share:..."`).
/// `None` se non resta nulla di utile (l'intera variabile andrebbe rimossa),
/// per lasciare a `xdg-open` il fallback di sistema piuttosto che impostare
/// una stringa vuota.
#[cfg(target_os = "linux")]
fn strip_appimage_data_dirs(value: &str, mount_dir: &str) -> Option<String> {
    let polluted = format!("{mount_dir}/usr/share");
    let cleaned: Vec<&str> = value.split(':').filter(|entry| entry.trim_end_matches('/') != polluted).collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.join(":"))
    }
}

/// Cartella dove l'AppImage è montata (contiene `usr/bin/<eseguibile>`) —
/// `None` fuori da un AppImage (es. installazione .deb/.rpm, dove
/// `XDG_DATA_DIRS` non è mai stata inquinata e non va toccata).
#[cfg(target_os = "linux")]
fn appimage_mount_dir_from(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    Some(exe.parent()?.parent()?.parent()?.to_path_buf())
}

#[cfg(target_os = "linux")]
fn xdg_open(arg: &str) {
    let mut command = std::process::Command::new("xdg-open");
    command.arg(arg);
    for var in APPIMAGE_POLLUTED_ENV_VARS {
        command.env_remove(var);
    }
    let mount_dir = std::env::current_exe().ok().and_then(|exe| appimage_mount_dir_from(&exe));
    if let (Some(mount_dir), Ok(data_dirs)) = (mount_dir, std::env::var("XDG_DATA_DIRS")) {
        match strip_appimage_data_dirs(&data_dirs, &mount_dir.to_string_lossy()) {
            Some(cleaned) => {
                command.env("XDG_DATA_DIRS", cleaned);
            }
            None => {
                command.env_remove("XDG_DATA_DIRS");
            }
        }
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

/// Apre nel file manager di sistema la cartella locale di un backup/bisync
/// — a differenza di `mounts::open_mount_folder`, disponibile a prescindere
/// dallo stato dell'ultima esecuzione: la cartella locale di un job esiste
/// sempre per costruzione, non richiede prima un'azione come "monta".
#[tauri::command]
pub fn open_local_folder(path: String) {
    open_path(&path);
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// Caso reale osservato dal vivo (Linux Mint/Cinnamon): il percorso di
    /// mount compare due volte, con e senza `/` finale, mescolato a voci
    /// legittime che vanno preservate — inclusa `/usr/share/cinnamon`, la
    /// cui assenza è la causa diretta del bug ("apri cartella" silenziosamente
    /// senza effetto, mentre gli URL funzionavano lo stesso perché i browser
    /// restano risolvibili anche dal solo fallback generico di glib).
    #[test]
    fn strip_appimage_data_dirs_keeps_legitimate_entries_and_drops_only_the_mount_path() {
        let value = "/tmp/.mount_rclonecBagCC/usr/share/:/tmp/.mount_rclonecBagCC/usr/share:/usr/share:/usr/share/cinnamon:/home/utente/.local/share/flatpak/exports/share:/var/lib/flatpak/exports/share:/usr/local/share:/usr/share";
        assert_eq!(
            strip_appimage_data_dirs(value, "/tmp/.mount_rclonecBagCC"),
            Some("/usr/share:/usr/share/cinnamon:/home/utente/.local/share/flatpak/exports/share:/var/lib/flatpak/exports/share:/usr/local/share:/usr/share".to_string())
        );
    }

    #[test]
    fn strip_appimage_data_dirs_returns_none_when_nothing_survives() {
        let value = "/tmp/.mount_x/usr/share/:/tmp/.mount_x/usr/share";
        assert_eq!(strip_appimage_data_dirs(value, "/tmp/.mount_x"), None);
    }

    #[test]
    fn strip_appimage_data_dirs_is_a_no_op_without_a_matching_mount_path() {
        let value = "/usr/share:/usr/local/share";
        assert_eq!(strip_appimage_data_dirs(value, "/tmp/.mount_x"), Some(value.to_string()));
    }

    #[test]
    fn appimage_mount_dir_from_goes_up_three_levels_from_the_executable() {
        let exe = std::path::Path::new("/tmp/.mount_rclonecBagCC/usr/bin/rclone-easy");
        assert_eq!(appimage_mount_dir_from(exe), Some(std::path::PathBuf::from("/tmp/.mount_rclonecBagCC")));
    }
}
