use std::path::Path;
use tokio::process::Command;

/// Nome del sidecar Tauri per rclone (`externalBin` in tauri.conf.json),
/// convenzione `<nome>-<target-triple>` — solo Linux x86_64 per ora, coerente
/// con la fase attuale solo-Linux del progetto (vedi SPEC.md).
const SIDECAR_NAME: &str = "rclone-x86_64-unknown-linux-gnu";

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
///    PATH di sistema, lo stesso comportamento di prima di questo slice.
fn resolve_rclone_binary_from(exe: &Path) -> String {
    if let Some(parent) = exe.parent() {
        let candidate = parent.join("rclone");
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
    let output = Command::new(resolve_rclone_binary())
        .arg("version")
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
