use std::path::Path;
use tokio::process::Command;

/// Su Windows, un eseguibile console (`rclone.exe`) lanciato da un'app GUI
/// senza questo flag apre sempre una finestra cmd visibile — anche per
/// invocazioni brevissime (`listremotes`, `config dump`), che producono un
/// flash percepibile, e per il demone `rcd` in particolare una finestra che
/// resta aperta per tutta la vita dell'app. Oltre al fastidio visivo, è
/// pericoloso: chiudere quella finestra con la X manda un evento che
/// termina il processo agganciato, uccidendo il demone da sotto i piedi
/// dell'app. `0x08000000` = `CREATE_NO_WINDOW`
/// (<https://learn.microsoft.com/windows/win32/procthread/process-creation-flags>).
/// Nessun equivalente necessario su Linux/macOS, dove i processi figli non
/// hanno mai una finestra propria.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Applica `CREATE_NO_WINDOW` a un `std::process::Command` — no-op sulle
/// altre piattaforme, così i chiamanti possono invocarla incondizionatamente.
pub(crate) fn hide_console_window(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Stessa cosa per `tokio::process::Command`, che espone lo stesso
/// `creation_flags` come metodo inerente (non tramite `CommandExt`) solo
/// sotto `cfg(windows)`.
pub(crate) fn hide_console_window_tokio(command: &mut Command) -> &mut Command {
    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// `true` se un processo con questo PID esiste ancora — usata per
/// distinguere un lock file di bisync genuinamente attivo da uno orfano
/// lasciato da un'esecuzione precedente terminata in modo anomalo (crash,
/// mancanza di corrente, terminazione forzata), vedi `bisync::clear_stale_lock`.
/// Best-effort: piattaforme non gestite esplicitamente (solo Linux e
/// Windows per ora) restituiscono sempre `true`, la scelta conservativa —
/// scambiare un lock genuinamente attivo per uno orfano e cancellarlo
/// sarebbe pericoloso, il contrario (non ripulire un lock davvero orfano)
/// è solo un inconveniente temporaneo.
#[cfg(target_os = "linux")]
pub(crate) fn process_is_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(target_os = "windows")]
pub(crate) fn process_is_alive(pid: u32) -> bool {
    let mut command = std::process::Command::new("tasklist");
    command.args(["/FI", &format!("PID eq {pid}"), "/NH"]);
    hide_console_window(&mut command);
    let Ok(output) = command.output() else { return true };
    String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) fn process_is_alive(_pid: u32) -> bool {
    true
}

/// Nome del sidecar Tauri per rclone (`externalBin` in tauri.conf.json),
/// convenzione `<nome>-<target-triple>[.exe su Windows]`.
#[cfg(target_os = "windows")]
const SIDECAR_NAME: &str = "rclone-x86_64-pc-windows-msvc.exe";
#[cfg(not(target_os = "windows"))]
const SIDECAR_NAME: &str = "rclone-x86_64-unknown-linux-gnu";

/// Nome che Tauri dà al sidecar dopo averlo copiato accanto all'eseguibile
/// principale in un'app pacchettizzata (senza più il suffisso di
/// target-triple) — `.exe` su Windows, nessuna estensione altrove.
#[cfg(target_os = "windows")]
const BUNDLED_NAME: &str = "rclone.exe";
#[cfg(not(target_os = "windows"))]
const BUNDLED_NAME: &str = "rclone";

/// Risolve il percorso del binario rclone da usare, dato il percorso
/// dell'eseguibile corrente — separata da `resolve_rclone_binary` solo per
/// poterla testare senza dipendere da `std::env::current_exe()` reale.
/// Tre casi, in ordine:
/// 1. app pacchettizzata: Tauri copia il sidecar accanto all'eseguibile
///    principale, già rinominato senza il suffisso di target-triple;
/// 2. sviluppo (`cargo tauri dev`/`cargo test`): l'eseguibile vive sotto
///    `src-tauri/target/<profilo>[/deps]/`, il sidecar sotto
///    `src-tauri/binaries/` — si risalgono gli antenati della cartella
///    invece di contare un numero fisso di livelli, perché la profondità
///    è diversa tra il binario dell'app e i binari di test;
/// 3. nessun sidecar trovato (repository appena clonato, prima di eseguire
///    `scripts/fetch-rclone-sidecar.sh`): ripiega sul comando `rclone` nel
///    PATH di sistema.
fn resolve_rclone_binary_from(exe: &Path) -> String {
    if let Some(parent) = exe.parent() {
        let candidate = parent.join(BUNDLED_NAME);
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }

    for ancestor in exe.ancestors() {
        let candidate = ancestor.join("binaries").join(SIDECAR_NAME);
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }

    "rclone".to_string()
}

pub(crate) fn resolve_rclone_binary() -> String {
    match std::env::current_exe() {
        Ok(exe) => resolve_rclone_binary_from(&exe),
        Err(_) => "rclone".to_string(),
    }
}

/// Estrae la versione dalla prima riga di `rclone version` (formato
/// `rclone v1.75.0`). Non usiamo `rclone version --check=false` per il
/// numero perché il formato testuale della prima riga è già stabile e
/// documentato da rclone stesso.
fn parse_version(stdout: &str) -> Result<String, String> {
    let first_line = stdout
        .lines()
        .next()
        .ok_or_else(|| "output di 'rclone version' vuoto".to_string())?;
    let version = first_line
        .strip_prefix("rclone ")
        .ok_or_else(|| format!("formato inatteso della prima riga: '{first_line}'"))?;
    Ok(version.trim().to_string())
}

async fn check_rclone_installed_in() -> Result<String, String> {
    let mut command = Command::new(resolve_rclone_binary());
    command.arg("version");
    let output = hide_console_window_tokio(&mut command)
        .output()
        .await
        .map_err(|e| format!("rclone non trovato: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "'rclone version' terminato con errore: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    parse_version(&String::from_utf8_lossy(&output.stdout))
}

#[tauri::command]
pub async fn check_rclone_installed() -> Result<String, String> {
    check_rclone_installed_in().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;

    #[cfg(target_os = "linux")]
    #[test]
    fn process_is_alive_is_true_for_our_own_pid() {
        assert!(process_is_alive(std::process::id()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_is_alive_is_false_for_a_pid_that_has_already_exited() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_is_alive(pid));
    }

    #[test]
    fn parse_version_extracts_version_from_first_line() {
        let stdout = "rclone v1.75.0\n- os/version: cachyos (64 bit)\n- os/type: linux\n";
        assert_eq!(parse_version(stdout).unwrap(), "v1.75.0");
    }

    #[test]
    fn parse_version_fails_on_empty_output() {
        assert!(parse_version("").is_err());
    }

    #[test]
    fn parse_version_fails_on_unexpected_format() {
        assert!(parse_version("qualcos'altro\n").is_err());
    }

    #[test]
    fn resolve_rclone_binary_from_finds_the_bundled_sidecar_next_to_the_executable() {
        let dir = TempDir::new("resolve-bundled");
        std::fs::create_dir_all(&dir.path).unwrap();
        let exe = dir.path.join("rclone-easy");
        std::fs::write(&exe, "").unwrap();
        let sidecar = dir.path.join("rclone");
        std::fs::write(&sidecar, "").unwrap();

        assert_eq!(resolve_rclone_binary_from(&exe), sidecar.to_string_lossy());
    }

    #[test]
    fn resolve_rclone_binary_from_finds_the_dev_sidecar_under_src_tauri_binaries() {
        let dir = TempDir::new("resolve-dev");
        let exe = dir.path.join("src-tauri").join("target").join("debug").join("deps").join("rclone_easy_lib-abc123");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        let binaries_dir = dir.path.join("src-tauri").join("binaries");
        std::fs::create_dir_all(&binaries_dir).unwrap();
        let sidecar = binaries_dir.join(SIDECAR_NAME);
        std::fs::write(&sidecar, "").unwrap();

        assert_eq!(resolve_rclone_binary_from(&exe), sidecar.to_string_lossy());
    }

    #[test]
    fn resolve_rclone_binary_from_falls_back_to_the_bare_command_when_nothing_is_found() {
        let dir = TempDir::new("resolve-fallback");
        std::fs::create_dir_all(&dir.path).unwrap();
        let exe = dir.path.join("rclone-easy");
        std::fs::write(&exe, "").unwrap();

        assert_eq!(resolve_rclone_binary_from(&exe), "rclone");
    }

    #[tokio::test]
    async fn check_rclone_installed_reports_a_version_when_binary_is_present() {
        // Trova il sidecar scaricato con scripts/fetch-rclone-sidecar.sh se
        // presente (caso normale in questo repo), altrimenti ripiega sul
        // rclone di sistema — richiede comunque che uno dei due esista
        // nell'ambiente di test.
        let result = check_rclone_installed_in().await;
        assert!(result.is_ok(), "atteso rclone disponibile (sidecar o di sistema) nell'ambiente di test: {result:?}");
        assert!(result.unwrap().starts_with('v'));
    }
}
