use std::collections::HashMap;
use tauri_plugin_autostart::ManagerExt;
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
/// **Bug reale trovato su Fedora 44/GNOME (16/9/2026)**: `xdg-desktop-portal-gnome`
/// tratta l'assenza della chiave `autostart` nella richiesta come un `false`
/// implicito, e a ogni `RequestBackground` concesso **cancella**
/// `~/.config/autostart/<app-id>.desktop` (bug noto, vedi
/// https://github.com/IsmaelMartinez/teams-for-linux/issues/2936). Dato che
/// questa funzione gira a ogni avvio dell'app, l'effetto era: l'app parte da
/// autostart, si registra presso il portale, GNOME cancella la voce di
/// autostart appena usata per farla partire, e al riavvio successivo la
/// sessione non trova più nulla da avviare — sparizione dalla tray "a ogni
/// riavvio", senza alcun legame con un aggiornamento di versione.
/// `heal_autostart_entry` (`lib.rs`) non copre questo caso perché la
/// cancellazione avviene in un task asincrono, dopo il suo controllo
/// sincrono, e comunque un'app che non riparte più non può auto-ripararsi.
///
/// **Fix**: leggere lo stato di autostart attuale (`tauri-plugin-autostart`,
/// la stessa fonte di verità di `heal_autostart_entry`) *prima* di chiamare
/// il portale e passarlo esplicitamente come opzione `autostart` — la
/// richiesta diventa così un no-op rispetto all'autostart invece di un reset
/// a `false`, sia che l'utente l'abbia attivato sia che non l'abbia mai
/// fatto. Corregge anche il rischio KDE già individuato in precedenza (`false`
/// esplicito veniva letto come "disattiva l'autostart esistente"): ora viene
/// passato `true` ogni volta che è davvero attivo, mai più un `false` che non
/// rispecchi lo stato reale.
pub fn request_background(app: &tauri::AppHandle) {
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    tauri::async_runtime::spawn(async move {
        if let Err(e) = try_request_background(autostart_enabled).await {
            eprintln!("impossibile registrarsi presso il portale Background (non bloccante): {e}");
        }
    });
}

async fn try_request_background(autostart_enabled: bool) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;

    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("reason", Value::from("Rclone Easy continua a funzionare per gestire i tuoi remote in background"));
    options.insert("autostart", Value::from(autostart_enabled));

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
