use std::collections::HashMap;
use zbus::zvariant::Value;

/// Registra l'app presso il portale "Background" di xdg-desktop-portal
/// (`org.freedesktop.portal.Background`, vedi
/// https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Background.html),
/// così desktop come GNOME la mostrano nella sezione "App in background"
/// quando la finestra è nascosta nella tray (`tray.rs`) — altrimenti, su un
/// desktop dove le tray icon "classiche" non sono disponibili (es. GNOME
/// senza l'estensione AppIndicator), l'app risulterebbe attiva ma del tutto
/// invisibile all'utente.
///
/// Best-effort e non bloccante: gira in un task separato, non impedisce
/// l'avvio dell'app se il portale non risponde (desktop senza
/// xdg-desktop-portal, o senza un backend che implementa questa interfaccia
/// specifica — non tutti i DE la supportano, KDE ad esempio non ne ha
/// comunque bisogno perché la tray "classica" già funziona lì). Non gestito
/// l'esito dell'eventuale prompt di conferma mostrato all'utente (non
/// servono permessi speciali nell'app per la sola comparsa nell'elenco).
pub fn request_background() {
    tauri::async_runtime::spawn(async {
        if let Err(e) = try_request_background().await {
            eprintln!("impossibile registrarsi presso il portale Background (non bloccante): {e}");
        }
    });
}

async fn try_request_background() -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;

    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("reason", Value::from("Rclone Easy continua a funzionare per gestire i tuoi remote in background"));
    options.insert("autostart", Value::from(false));

    connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Background"),
            "RequestBackground",
            &("", options),
        )
        .await?;

    Ok(())
}
