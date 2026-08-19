/// Rileva come questa istanza dell'app è stata installata, per decidere se
/// l'aggiornamento in-app (plugin updater di Tauri) è utilizzabile
/// direttamente o se bisogna solo segnalare che esiste una versione più
/// recente e rimandare alla pagina di download: l'updater di Tauri sa
/// sostituire un AppImage o un installer NSIS di Windows, ma non un
/// pacchetto `.deb`/`.rpm` già installato dal gestore pacchetti di sistema
/// (che si aggiorna per conto proprio, es. `apt`/`dnf`).
#[tauri::command]
pub fn installation_kind() -> String {
    #[cfg(target_os = "windows")]
    {
        "windows".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        // Variabile d'ambiente impostata dal runtime AppImage stesso
        // (`AppRun`) per ogni processo lanciato da un'immagine montata —
        // assente per un binario installato normalmente da .deb/.rpm.
        if std::env::var_os("APPIMAGE").is_some() {
            "appimage".to_string()
        } else {
            "package-manager".to_string()
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        "package-manager".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_kind_returns_a_known_value() {
        let kind = installation_kind();
        assert!(["windows", "appimage", "package-manager"].contains(&kind.as_str()));
    }
}
