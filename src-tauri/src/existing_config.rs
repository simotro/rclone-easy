use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;

/// Aggiunge `--config <path>` solo se è stato scelto esplicitamente un file
/// diverso da quello di sistema (selettore file in "Importa remote esistenti"
/// — SPEC: l'utente può scegliere un rclone.conf da una cartella qualsiasi,
/// non solo quello di default). Senza questo flag rclone usa la propria
/// risoluzione standard (`RCLONE_CONFIG` o `~/.config/rclone/rclone.conf`),
/// comportamento invariato per chi non lo specifica.
fn apply_config_path<'a>(command: &'a mut Command, config_path: Option<&Path>) -> &'a mut Command {
    match config_path {
        Some(path) => command.args(["--config", &path.to_string_lossy()]),
        None => command,
    }
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExistingRemote {
    pub name: String,
    pub kind: String,
}

/// Parsa l'output di `rclone listremotes --long`, una riga per remote nel
/// formato `nome:    tipo` (colonne allineate con spazi, non tab) — formato
/// stabile documentato da rclone. Righe vuote o non conformi vengono
/// ignorate piuttosto che far fallire l'intera lettura.
fn parse_listremotes(stdout: &str) -> Vec<ExistingRemote> {
    stdout
        .lines()
        .filter_map(|line| {
            let (name, kind) = line.split_once(':')?;
            let name = name.trim();
            let kind = kind.trim();
            if name.is_empty() || kind.is_empty() {
                return None;
            }
            Some(ExistingRemote { name: name.to_string(), kind: kind.to_string() })
        })
        .collect()
}

async fn detect_existing_remotes_in(config_path: Option<&Path>) -> Result<Vec<ExistingRemote>, String> {
    let mut command = Command::new(crate::rclone_bin::resolve_rclone_binary());
    command.args(["listremotes", "--long"]);
    apply_config_path(&mut command, config_path);
    let output = command.output().await.map_err(|e| format!("impossibile eseguire 'rclone listremotes': {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "'rclone listremotes' terminato con errore: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(parse_listremotes(&String::from_utf8_lossy(&output.stdout)))
}

#[tauri::command]
pub async fn detect_existing_remotes(config_path: Option<String>) -> Result<Vec<ExistingRemote>, String> {
    detect_existing_remotes_in(config_path.as_deref().map(Path::new)).await
}

/// `rclone config file` stampa una riga di spiegazione seguita dal percorso
/// (varia se il file esiste già o no, es. "Configuration file is stored
/// at:" oppure "Configuration file doesn't exist, but rclone will use this
/// path:") — l'unica cosa stabile è che il percorso è l'ultima riga non
/// vuota, indipendentemente dal testo prima.
fn extract_config_file_path(stdout: &str) -> Option<&str> {
    stdout.lines().map(str::trim).filter(|l| !l.is_empty()).last()
}

/// Percorso del `rclone.conf` che rclone userebbe di default (rispetta
/// `RCLONE_CONFIG` se impostata) — usato dal frontend come cartella di
/// partenza del selettore file in "Importa remote esistenti", così l'utente
/// parte già dalla posizione standard invece di dover navigare a mano.
/// `Ok(None)` se rclone segnala che il file non esiste ancora (nessuna
/// installazione rclone precedente su questa macchina): non è un errore,
/// il selettore parte semplicemente senza percorso preselezionato.
#[tauri::command]
pub async fn standard_rclone_config_path() -> Result<Option<String>, String> {
    let output = Command::new(crate::rclone_bin::resolve_rclone_binary())
        .args(["config", "file"])
        .output()
        .await
        .map_err(|e| format!("impossibile eseguire 'rclone config file': {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "'rclone config file' terminato con errore: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(path) = extract_config_file_path(&stdout) else { return Ok(None) };
    Ok(Path::new(path).is_file().then(|| path.to_string()))
}

/// Converte un valore JSON di `config dump` (stringa, bool o numero — mai
/// altro per un backend rclone) nella sua forma testuale, per finire nella
/// stessa mappa `HashMap<String, String>` che `config/create` si aspetta.
fn json_value_to_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Estrae tipo e parametri di un singolo remote dall'output di
/// `rclone config dump` (oggetto JSON `{"nome": {"type": ..., "campo": ...}}`).
/// I valori dei campi (comprese eventuali password) sono quelli già
/// "obscured" così come salvati su disco, non testo in chiaro — vanno
/// ripassati a `config/create` con `opt.noObscure: true`, mai con
/// `opt.obscure: true` (che li offuscherebbe una seconda volta, producendo
/// credenziali sbagliate senza errore visibile).
pub(crate) fn extract_remote_parameters(dump: &serde_json::Value, name: &str) -> Result<(String, HashMap<String, String>), String> {
    let entry = dump
        .get(name)
        .and_then(|v| v.as_object())
        .ok_or_else(|| format!("remote '{name}' non trovato nel rclone.conf di sistema"))?;

    let mut parameters: HashMap<String, String> =
        entry.iter().filter_map(|(k, v)| json_value_to_string(v).map(|s| (k.clone(), s))).collect();

    let kind = parameters
        .remove("type")
        .ok_or_else(|| format!("campo 'type' mancante per il remote '{name}' nel rclone.conf di sistema"))?;

    Ok((kind, parameters))
}

async fn dump_remote_parameters_in(
    name: &str,
    config_path: Option<&Path>,
) -> Result<(String, HashMap<String, String>), String> {
    let mut command = Command::new(crate::rclone_bin::resolve_rclone_binary());
    command.args(["config", "dump"]);
    apply_config_path(&mut command, config_path);
    let output = command.output().await.map_err(|e| format!("impossibile eseguire 'rclone config dump': {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "'rclone config dump' terminato con errore: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let dump: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("output di 'rclone config dump' non interpretabile: {e}"))?;

    extract_remote_parameters(&dump, name)
}

/// Non è un comando Tauri: i parametri (comprese eventuali password
/// offuscate) non devono mai attraversare il confine verso il frontend. Il
/// flusso di import (`remotes::import_remote`) seleziona per nome e usa
/// questa funzione internamente per recuperare/reiniettare i valori.
pub(crate) async fn dump_remote_parameters(
    name: &str,
    config_path: Option<&Path>,
) -> Result<(String, HashMap<String, String>), String> {
    dump_remote_parameters_in(name, config_path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_config_file_path_reads_the_last_non_empty_line_when_the_file_exists() {
        let stdout = "Configuration file is stored at:\n/home/utente/.config/rclone/rclone.conf\n";
        assert_eq!(extract_config_file_path(stdout), Some("/home/utente/.config/rclone/rclone.conf"));
    }

    #[test]
    fn extract_config_file_path_reads_the_last_line_when_the_file_does_not_exist_yet() {
        let stdout = "Configuration file doesn't exist, but rclone will use this path:\n/home/utente/.config/rclone/rclone.conf\n";
        assert_eq!(extract_config_file_path(stdout), Some("/home/utente/.config/rclone/rclone.conf"));
    }

    #[test]
    fn extract_config_file_path_is_none_for_empty_output() {
        assert_eq!(extract_config_file_path(""), None);
    }

    #[test]
    fn apply_config_path_adds_the_flag_only_when_a_path_is_given() {
        let mut with_path = Command::new("true");
        apply_config_path(&mut with_path, Some(Path::new("/tmp/altro.conf")));
        let args: Vec<_> = with_path.as_std().get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(args, vec!["--config", "/tmp/altro.conf"]);

        let mut without_path = Command::new("true");
        apply_config_path(&mut without_path, None);
        let args: Vec<_> = without_path.as_std().get_args().collect();
        assert!(args.is_empty());
    }

    #[test]
    fn parse_listremotes_extracts_name_and_kind() {
        let stdout = "drive-copass: s3   \nmega-test:    mega\n";
        assert_eq!(
            parse_listremotes(stdout),
            vec![
                ExistingRemote { name: "drive-copass".to_string(), kind: "s3".to_string() },
                ExistingRemote { name: "mega-test".to_string(), kind: "mega".to_string() },
            ]
        );
    }

    #[test]
    fn parse_listremotes_returns_empty_vec_for_no_remotes() {
        assert_eq!(parse_listremotes(""), Vec::new());
    }

    #[test]
    fn parse_listremotes_ignores_malformed_lines() {
        let stdout = "riga senza due punti\ndrive-copass: s3\n";
        assert_eq!(
            parse_listremotes(stdout),
            vec![ExistingRemote { name: "drive-copass".to_string(), kind: "s3".to_string() }]
        );
    }

    #[tokio::test]
    async fn detect_existing_remotes_reads_the_real_rclone_conf() {
        // Sull'ambiente di sviluppo esistono già remote reali (drive-copass,
        // mega-test) nel rclone.conf di sistema: verifichiamo solo che la
        // chiamata riesca e produca una lista, senza legarci a nomi
        // specifici che potrebbero cambiare.
        let result = detect_existing_remotes_in(None).await;
        assert!(result.is_ok(), "atteso successo anche con lista vuota: {result:?}");
    }

    fn sample_dump() -> serde_json::Value {
        serde_json::json!({
            "finto": {
                "type": "mega",
                "user": "a@b.com",
                "pass": "offuscata-finta",
                "hard_delete": true,
                "some_number": 42
            }
        })
    }

    #[test]
    fn extract_remote_parameters_splits_type_from_the_rest() {
        let (kind, parameters) = extract_remote_parameters(&sample_dump(), "finto").unwrap();
        assert_eq!(kind, "mega");
        assert_eq!(parameters.get("user"), Some(&"a@b.com".to_string()));
        assert_eq!(parameters.get("pass"), Some(&"offuscata-finta".to_string()));
        assert!(!parameters.contains_key("type"));
    }

    #[test]
    fn extract_remote_parameters_converts_non_string_json_values() {
        let (_, parameters) = extract_remote_parameters(&sample_dump(), "finto").unwrap();
        assert_eq!(parameters.get("hard_delete"), Some(&"true".to_string()));
        assert_eq!(parameters.get("some_number"), Some(&"42".to_string()));
    }

    #[test]
    fn extract_remote_parameters_fails_when_remote_missing() {
        let result = extract_remote_parameters(&sample_dump(), "non-esiste");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn dump_remote_parameters_reads_a_real_remote_when_one_exists() {
        // Non si può assumere che un remote specifico esista nel rclone.conf
        // reale di sviluppo (può cambiare, ed è cambiato durante lo
        // sviluppo di questo stesso slice) — si interroga prima
        // detect_existing_remotes_in per sapere cosa c'è davvero, senza mai
        // legare il test a un nome fisso.
        let existing = detect_existing_remotes_in(None).await.unwrap();
        let Some(remote) = existing.first() else {
            return; // nessun remote nel config di sistema: nulla da verificare qui.
        };

        let result = dump_remote_parameters_in(&remote.name, None).await;
        assert!(result.is_ok(), "atteso successo per un remote noto: {result:?}");
        let (kind, _parameters) = result.unwrap();
        assert_eq!(kind, remote.kind);
    }

    #[tokio::test]
    async fn dump_remote_parameters_fails_cleanly_for_an_unknown_name() {
        let result = dump_remote_parameters_in("rclone-easy-nome-che-non-esiste-xyz", None).await;
        assert!(result.is_err());
    }
}
