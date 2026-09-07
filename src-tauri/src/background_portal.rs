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
///
/// Nessuna opzione `autostart` nella richiesta: l'avvio automatico è già
/// gestito direttamente da `tauri-plugin-autostart` (voce XDG scritta di
/// suo in `~/.config/autostart/`, vedi `heal_autostart_entry` in `lib.rs`),
/// non ha bisogno che sia anche il portale a occuparsene. Passare
/// esplicitamente `false` rischia di far interpretare a un backend del
/// portale (es. `xdg-desktop-portal-kde`, che sembra gestire "in
/// background"/"avvio automatico" come permessi legati alla stessa voce)
/// questa richiesta come "disattiva l'autostart esistente", non solo come
/// "non chiedermelo tu" — rischio concreto specialmente quando il portale
/// rivaluta l'identità dell'app (es. dopo un aggiornamento che sostituisce
/// il binario).
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
