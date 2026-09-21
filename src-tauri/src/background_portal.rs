use std::collections::HashMap;
use std::path::PathBuf;
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
/// comunque bisogno perché la tray "classica" già funziona lì).
///
/// Ogni `RequestBackground` concesso riscrive anche l'autostart, e per un'app
/// non Flatpak il portale usa lo stesso file di `tauri-plugin-autostart`
/// (`~/.config/autostart/<app-id>.desktop`, con `<app-id>` ricavato dal nome
/// dello scope systemd con cui la sessione ha lanciato l'app):
/// - senza la chiave `autostart` la voce viene **cancellata**;
/// - con `autostart: true` ma senza `commandline` viene riscritta **senza
///   `Exec=`**, quindi la sessione non avvierebbe più nulla pur risultando
///   "attiva" a `is_enabled()`, che controlla solo l'esistenza del file.
///
/// Per questo la richiesta passa sempre lo stato reale dell'autostart e, se
/// attivo, lo stesso comando scritto da `tauri-plugin-autostart`: entrambi i
/// writer producono una voce valida e l'ordine in cui arrivano non conta.
///
/// Il portale identifica l'app dallo scope del processo: un avvio da terminale
/// (es. `npm run tauri dev`) erediterebbe quello del terminale, e la richiesta
/// cancellerebbe o creerebbe la voce di autostart di un'altra applicazione.
/// La richiesta parte quindi solo se lo scope è quello di rclone-easy.
pub fn request_background(app: &tauri::AppHandle) {
    if !runs_in_own_scope() {
        return;
    }
    let commandline = if app.autolaunch().is_enabled().unwrap_or(false) {
        match autostart_command() {
            Some(command) => Some(command),
            None => return,
        }
    } else {
        None
    };
    tauri::async_runtime::spawn(async move {
        if let Err(e) = try_request_background(commandline).await {
            eprintln!("impossibile registrarsi presso il portale Background (non bloccante): {e}");
        }
    });
}

async fn try_request_background(commandline: Option<String>) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;

    connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Background"),
            "RequestBackground",
            &("", background_options(commandline)),
        )
        .await?;

    Ok(())
}

fn background_options(commandline: Option<String>) -> HashMap<&'static str, Value<'static>> {
    let mut options: HashMap<&'static str, Value<'static>> = HashMap::new();
    options.insert("reason", Value::from("Rclone Easy continua a funzionare per gestire i tuoi remote in background"));
    options.insert("autostart", Value::from(commandline.is_some()));
    if let Some(command) = commandline {
        options.insert("commandline", Value::from(vec![command]));
    }
    options
}

/// Stesso eseguibile che `tauri-plugin-autostart` scrive in `Exec=`: per
/// un'AppImage il file `$APPIMAGE`, non il binario nel mount temporaneo.
/// Tra virgolette perché il portale unisce gli argomenti con spazi senza
/// alcun quoting.
fn autostart_command() -> Option<String> {
    let executable = std::env::var_os("APPIMAGE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())?;
    Some(format!("\"{}\"", executable.display()))
}

fn runs_in_own_scope() -> bool {
    std::fs::read_to_string("/proc/self/cgroup").map(|cgroup| cgroup_belongs_to_app(&cgroup)).unwrap_or(false)
}

/// Unità nel formato `app[-<launcher>]-<id>-<random>.scope`, o `...@<random>.service`
/// per gli avvii automatici (vedi https://systemd.io/DESKTOP_ENVIRONMENTS/),
/// con `-` dell'id scritto come `\x2d`.
fn cgroup_belongs_to_app(cgroup: &str) -> bool {
    cgroup.lines().filter_map(|line| line.rsplit('/').next()).any(|unit| {
        let unit = unit.replace("\\x2d", "-");
        unit.starts_with("app-") && ["-rclone-easy-", "-rclone-easy@", "-rclone-easy."].iter().any(|marker| unit.contains(marker))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_autostart_the_request_states_it_explicitly_and_omits_the_command() {
        let options = background_options(None);
        assert_eq!(options.get("autostart"), Some(&Value::from(false)));
        assert!(!options.contains_key("commandline"));
    }

    #[test]
    fn with_autostart_the_request_passes_the_command_so_the_entry_keeps_its_exec() {
        let options = background_options(Some("\"/home/utente/rclone-easy\"".to_string()));
        assert_eq!(options.get("autostart"), Some(&Value::from(true)));
        assert_eq!(options.get("commandline"), Some(&Value::from(vec!["\"/home/utente/rclone-easy\"".to_string()])));
    }

    #[test]
    fn menu_launch_scope_is_recognised() {
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-rclone\\x2deasy-75964.scope\n";
        assert!(cgroup_belongs_to_app(cgroup));
    }

    #[test]
    fn autostart_service_is_recognised() {
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-rclone\\x2deasy@autostart.service\n";
        assert!(cgroup_belongs_to_app(cgroup));
    }

    #[test]
    fn a_terminal_scope_is_not_ours() {
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-org.gnome.Ptyxis-1234.scope\n";
        assert!(!cgroup_belongs_to_app(cgroup));
    }

    #[test]
    fn a_session_without_app_scope_is_not_ours() {
        assert!(!cgroup_belongs_to_app("0::/user.slice/user-1000.slice/session-3.scope\n"));
    }
}
