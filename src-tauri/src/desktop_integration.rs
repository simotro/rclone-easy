use std::path::{Path, PathBuf};

/// Icona SVG bundlata nel binario stesso (non letta dal filesystem
/// dell'AppImage montata, `$APPDIR/usr/share/icons/...`): quel percorso
/// esiste solo mentre l'AppImage è in esecuzione, ma la voce di menu deve
/// restare valida anche a processo chiuso — l'icona va quindi copiata in una
/// posizione persistente sotto la home dell'utente.
const ICON_SVG: &[u8] = include_bytes!("../icons/icon.svg");

/// Scrive/aggiorna una voce nel menu applicazioni per l'AppImage in
/// esecuzione — per come è fatto il formato, un'AppImage lanciata
/// direttamente non si integra mai da sola nel sistema (a differenza dei
/// pacchetti `.deb`/`.rpm`, dove il bundler Tauri genera già un `.desktop`
/// dentro il pacchetto stesso). Nessun'azione per quegli altri due canali di
/// installazione.
///
/// Rieseguita ad ogni avvio, non solo alla prima: se la voce esiste già e
/// punta all'AppImage nella sua posizione attuale non viene riscritta
/// (nessun I/O superfluo); se manca, o punta a un percorso diverso da dove
/// l'AppImage gira ora (es. l'utente l'ha spostata o rinominata), viene
/// aggiornata — l'integrazione si "auto-ripara" senza richiedere
/// un'azione esplicita dall'utente.
///
/// Best-effort e non bloccante: nessun errore qui deve impedire l'avvio
/// dell'app (cartella dati non scrivibile, `$HOME` non determinabile, ecc.).
pub fn ensure_installed() {
    if let Err(e) = try_ensure_installed() {
        eprintln!("impossibile aggiungere la voce al menu applicazioni (non bloccante): {e}");
    }
}

fn try_ensure_installed() -> Result<(), String> {
    // Variabile impostata dal runtime AppImage stesso (`AppRun`) col
    // percorso assoluto del file .AppImage in esecuzione — assente per
    // un'installazione .deb/.rpm, unico segnale già usato altrove
    // (`installation.rs::installation_kind`) per distinguere i tre canali.
    let Some(appimage_path) = std::env::var_os("APPIMAGE") else { return Ok(()) };
    let appimage_path = appimage_path.to_string_lossy().into_owned();

    let data_home = xdg_data_home()?;
    let icon_path = install_icon(&data_home)?;
    install_desktop_entry(&data_home, &appimage_path, &icon_path)
}

fn xdg_data_home_from(xdg_data_home: Option<&str>, home: Option<&str>) -> Result<PathBuf, String> {
    if let Some(dir) = xdg_data_home {
        return Ok(PathBuf::from(dir));
    }
    let home = home.ok_or_else(|| "variabile HOME non impostata".to_string())?;
    Ok(PathBuf::from(home).join(".local/share"))
}

fn xdg_data_home() -> Result<PathBuf, String> {
    xdg_data_home_from(std::env::var("XDG_DATA_HOME").ok().as_deref(), std::env::var("HOME").ok().as_deref())
}

fn install_icon(data_home: &Path) -> Result<PathBuf, String> {
    let icon_dir = data_home.join("icons/hicolor/scalable/apps");
    std::fs::create_dir_all(&icon_dir).map_err(|e| format!("impossibile creare '{}': {e}", icon_dir.display()))?;
    let icon_path = icon_dir.join("rclone-easy.svg");
    // Riscritta sempre: costo trascurabile (poche righe di SVG), copre anche
    // un'icona cambiata in una versione futura senza logica di confronto ad
    // hoc in più.
    std::fs::write(&icon_path, ICON_SVG).map_err(|e| format!("impossibile scrivere '{}': {e}", icon_path.display()))?;
    Ok(icon_path)
}

fn desktop_entry_contents(appimage_path: &str, icon_path: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=rclone-easy\n\
         Comment=Interfaccia grafica per rclone\n\
         Exec=\"{appimage_path}\"\n\
         Icon={icon_path}\n\
         Terminal=false\n\
         Categories=Utility;\n"
    )
}

fn install_desktop_entry(data_home: &Path, appimage_path: &str, icon_path: &Path) -> Result<(), String> {
    let applications_dir = data_home.join("applications");
    std::fs::create_dir_all(&applications_dir).map_err(|e| format!("impossibile creare '{}': {e}", applications_dir.display()))?;
    let desktop_path = applications_dir.join("rclone-easy.desktop");

    let contents = desktop_entry_contents(appimage_path, &icon_path.to_string_lossy());
    let up_to_date = std::fs::read_to_string(&desktop_path).map(|existing| existing == contents).unwrap_or(false);
    if up_to_date {
        return Ok(());
    }
    std::fs::write(&desktop_path, contents).map_err(|e| format!("impossibile scrivere '{}': {e}", desktop_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_contents_points_exec_at_the_appimage_and_icon_at_the_installed_copy() {
        let contents = desktop_entry_contents("/home/utente/App Images/rclone-easy.AppImage", "/home/utente/.local/share/icons/hicolor/scalable/apps/rclone-easy.svg");
        assert!(contents.contains("Exec=\"/home/utente/App Images/rclone-easy.AppImage\"\n"));
        assert!(contents.contains("Icon=/home/utente/.local/share/icons/hicolor/scalable/apps/rclone-easy.svg\n"));
        assert!(contents.starts_with("[Desktop Entry]\n"));
    }

    #[test]
    fn xdg_data_home_from_prefers_the_explicit_variable_when_set() {
        assert_eq!(xdg_data_home_from(Some("/custom/data"), Some("/home/utente")).unwrap(), PathBuf::from("/custom/data"));
    }

    #[test]
    fn xdg_data_home_from_falls_back_to_home_local_share() {
        assert_eq!(xdg_data_home_from(None, Some("/home/utente")).unwrap(), PathBuf::from("/home/utente/.local/share"));
    }

    #[test]
    fn xdg_data_home_from_fails_without_either_variable() {
        assert!(xdg_data_home_from(None, None).is_err());
    }

    #[test]
    fn install_desktop_entry_heals_itself_when_the_appimage_path_changes() {
        use crate::rcd::tests::TempDir;

        let data_home = TempDir::new("desktop-integration");
        let icon_path = data_home.path.join("icon.svg");
        let desktop_path = data_home.path.join("applications").join("rclone-easy.desktop");

        install_desktop_entry(&data_home.path, "/home/utente/rclone-easy.AppImage", &icon_path).unwrap();
        let first_write = std::fs::read_to_string(&desktop_path).unwrap();
        assert!(first_write.contains("/home/utente/rclone-easy.AppImage"));

        // Percorso invariato: nessuna riscrittura necessaria, ma comunque
        // idempotente se capitasse.
        install_desktop_entry(&data_home.path, "/home/utente/rclone-easy.AppImage", &icon_path).unwrap();
        assert_eq!(std::fs::read_to_string(&desktop_path).unwrap(), first_write);

        // L'utente ha spostato/rinominato l'AppImage: la voce si aggiorna
        // da sé al giro successivo, invece di restare orfana.
        install_desktop_entry(&data_home.path, "/home/utente/Scaricati/rclone-easy_nuovo.AppImage", &icon_path).unwrap();
        let updated = std::fs::read_to_string(&desktop_path).unwrap();
        assert!(updated.contains("/home/utente/Scaricati/rclone-easy_nuovo.AppImage"));
        assert!(!updated.contains("/home/utente/rclone-easy.AppImage\""));
    }

    #[test]
    fn install_icon_writes_the_bundled_svg() {
        use crate::rcd::tests::TempDir;

        let data_home = TempDir::new("desktop-integration-icon");
        let icon_path = install_icon(&data_home.path).unwrap();
        assert_eq!(std::fs::read(&icon_path).unwrap(), ICON_SVG);
    }
}
