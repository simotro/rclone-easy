use crate::rcd::RcdState;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Prefisso del nome della cartella "fratella" di una destinazione dove
/// backup e bisync spostano (invece di cancellare per sempre) i file
/// rimossi o sovrascritti — via `--backup-dir` di rclone (RC: `BackupDir`;
/// bisync: `--backup-dir1`/`--backup-dir2`).
const TRASH_FOLDER_PREFIX: &str = ".rclone-easy-trash-";

/// Marcatore fisso incluso in ogni suffisso assegnato da questa app (via
/// `--suffix`) ai file spostati nel cestino — permette di riconoscere una
/// voce del cestino e di ricostruirne il nome originale, distinguendola da
/// un file che si chiamasse così per coincidenza.
const SUFFIX_MARKER: &str = ".rce-trash-";

/// Giorni di conservazione prima della cancellazione definitiva automatica —
/// un solo numero fisso invece di un pannello di opzioni: abbastanza per
/// accorgersi di un errore senza far crescere lo spazio occupato
/// all'infinito. Concordato con Simone (21/8/2026): 30 giorni.
const RETENTION_DAYS: u64 = 30;

/// `(nome_remote, percorso_senza_prefisso)` — `nome_remote` è `None` per un
/// percorso locale. Stessa logica di `jobs::remote_name_of`/
/// `bisync::remote_name_of` (non riusabile da qui: privata in entrambi i
/// moduli), consapevole della piattaforma tramite `Path::is_absolute` per
/// non scambiare una lettera di unità Windows (`C:\...`) per un remote.
fn split_fs(fs: &str) -> (Option<&str>, &str) {
    if Path::new(fs).is_absolute() {
        (None, fs)
    } else if let Some((name, rest)) = fs.split_once(':') {
        (Some(name), rest)
    } else {
        (None, fs)
    }
}

/// Cartella "fratella" di `dest` (stesso genitore, mai annidata dentro
/// `dest` stesso) dove backup/bisync spostano ciò che verrebbe
/// cancellato/sovrascritto. Necessariamente fratella e non annidata:
/// verificato empiricamente (21/8/2026) che rclone rifiuta un `--backup-dir`
/// contenuto nella destinazione stessa ("destination and parameter to
/// --backup-dir mustn't overlap"). `None` solo nel caso limite in cui `dest`
/// è già una radice (locale `"/"` o di un remote `"remoto:"`): lì non esiste
/// alcun percorso fratello sullo stesso remote che non finisca comunque
/// dentro `dest` — in quel caso il job procede senza questa protezione
/// aggiuntiva (per il backup resta comunque attivo `MaxDelete`, per bisync
/// il blocco nativo "too many deletes").
pub(crate) fn trash_fs_for(dest: &str) -> Option<String> {
    let (remote, path) = split_fs(dest);
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return None;
    }
    let basename = path.rsplit('/').next().unwrap_or(path);
    // Tutto ciò che precede `basename` in `path`, separatore incluso se
    // presente (es. "casa/foto" -> "casa/"), vuoto se `path` è un solo
    // livello sotto la radice (remota: "foto" -> ""; locale: "/foto" -> "/",
    // il separatore iniziale resta perché fa parte di `path` stesso).
    let parent = &path[..path.len() - basename.len()];
    let sibling = format!("{parent}{TRASH_FOLDER_PREFIX}{basename}");
    Some(match remote {
        Some(remote) => format!("{remote}:{sibling}"),
        None => sibling,
    })
}

/// Suffisso da passare a `--suffix`/`Suffix` per una singola esecuzione —
/// timestamp Unix incluso così più versioni dello stesso file nel tempo non
/// si sovrascrivono a vicenda nel cestino.
pub(crate) fn trash_suffix(now_unix: u64) -> String {
    format!("{SUFFIX_MARKER}{now_unix}")
}

/// Da un nome di file già passato per `--suffix-keep-extension` (suffisso
/// inserito prima dell'estensione, es. `"foto.rce-trash-1755765000.jpg"`,
/// oppure `"README.rce-trash-1755765000"` se il file originale non aveva
/// estensione) ricostruisce `(nome_originale, timestamp)`. `None` se `name`
/// non è una voce del cestino di questa app (nessun marcatore trovato).
fn parse_trashed_name(name: &str) -> Option<(String, u64)> {
    let idx = name.find(SUFFIX_MARKER)?;
    let stem = &name[..idx];
    let rest = &name[idx + SUFFIX_MARKER.len()..];
    let (timestamp_str, extension) = match rest.split_once('.') {
        Some((ts, ext)) => (ts, Some(ext)),
        None => (rest, None),
    };
    let when_unix: u64 = timestamp_str.parse().ok()?;
    let original_name = match extension {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem.to_string(),
    };
    Some((original_name, when_unix))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    /// Percorso relativo dentro il cestino (da ripassare a
    /// `restore_trash_entry` per identificare la voce).
    pub trash_path: String,
    /// Percorso relativo ricostruito di dove il file verrebbe ripristinato,
    /// relativo alla destinazione originale (non al cestino).
    pub original_path: String,
    pub when_unix: u64,
    pub size: i64,
}

fn extract_list(body: &serde_json::Value) -> Result<Vec<serde_json::Value>, String> {
    body.get("list")
        .and_then(|v| v.as_array())
        .cloned()
        .ok_or_else(|| format!("campo 'list' mancante nella risposta di rclone rcd: {body}"))
}

/// Elenca il contenuto del cestino di `dest` — solo le voci riconosciute
/// come proprie di questa app (`parse_trashed_name`); qualunque altro file
/// eventualmente presente nella cartella (non dovrebbe succedere, ma non
/// vale la pena farne un errore) viene ignorato invece di rompere l'intera
/// lista. Lista vuota (non un errore) se `dest` è un caso limite senza
/// cestino possibile (`trash_fs_for` -> `None`) o se il cestino non esiste
/// ancora (nessuna cancellazione/sovrascrittura è mai avvenuta).
pub(crate) async fn list_trash_in(state: &RcdState, dest: &str) -> Result<Vec<TrashEntry>, String> {
    let Some(trash_fs) = trash_fs_for(dest) else { return Ok(Vec::new()) };
    let body = crate::rcd::call(
        state,
        "operations/list",
        serde_json::json!({
            "fs": trash_fs,
            "remote": "",
            "opt": { "filesOnly": true, "recurse": true, "noModTime": false },
        }),
    )
    .await;
    let body = match body {
        Ok(body) => body,
        Err(e) if e.contains("directory not found") => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut entries = Vec::new();
    for item in extract_list(&body)? {
        let path = item.get("Path").and_then(|v| v.as_str()).unwrap_or_default();
        let name = item.get("Name").and_then(|v| v.as_str()).unwrap_or_default();
        let Some((original_name, when_unix)) = parse_trashed_name(name) else { continue };
        let size = item.get("Size").and_then(|v| v.as_i64()).unwrap_or(0);
        let dir_prefix = path.strip_suffix(name).unwrap_or("");
        entries.push(TrashEntry {
            trash_path: path.to_string(),
            original_path: format!("{dir_prefix}{original_name}"),
            when_unix,
            size,
        });
    }
    entries.sort_by(|a, b| b.when_unix.cmp(&a.when_unix));
    Ok(entries)
}

/// Ripristina una voce del cestino nella sua posizione originale sotto
/// `dest`, poi la rimuove dal cestino — comportamento tipico di un cestino
/// "ripristina" (non lascia una copia doppia). Se la posizione originale ha
/// nel frattempo un file diverso, `operations/copyfile` lo sovrascrive senza
/// chiedere conferma: è una scelta esplicita dell'utente cliccando
/// "Ripristina" su quella voce precisa, non un'operazione automatica.
pub(crate) async fn restore_entry_in(state: &RcdState, dest: &str, trash_path: &str, original_path: &str) -> Result<(), String> {
    let trash_fs = trash_fs_for(dest).ok_or_else(|| "questa destinazione non ha un cestino".to_string())?;
    crate::rcd::call(
        state,
        "operations/copyfile",
        serde_json::json!({
            "srcFs": trash_fs,
            "srcRemote": trash_path,
            "dstFs": dest,
            "dstRemote": original_path,
        }),
    )
    .await?;
    crate::rcd::call(state, "operations/deletefile", serde_json::json!({ "fs": trash_fs, "remote": trash_path })).await?;
    Ok(())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Cancella per sempre le voci del cestino di `dest` più vecchie di
/// `RETENTION_DAYS` — chiamata a fine di ogni run riuscito (backup e
/// bisync). Deliberatamente "best effort": un errore nel ripulire il
/// cestino (es. remote temporaneamente irraggiungibile) non deve far
/// fallire un backup/bisync altrimenti riuscito, quindi gli errori per
/// singola voce vengono scartati invece di propagati. Non early-return su un
/// errore di `list_trash_in` per lo stesso motivo.
pub(crate) async fn cleanup_old_entries_in(state: &RcdState, dest: &str) {
    let Some(trash_fs) = trash_fs_for(dest) else { return };
    let Ok(entries) = list_trash_in(state, dest).await else { return };
    let cutoff = now_unix().saturating_sub(RETENTION_DAYS * 24 * 60 * 60);
    for entry in entries.into_iter().filter(|e| e.when_unix < cutoff) {
        let _ = crate::rcd::call(state, "operations/deletefile", serde_json::json!({ "fs": trash_fs, "remote": entry.trash_path })).await;
    }
}

#[tauri::command]
pub async fn list_trash(state: tauri::State<'_, RcdState>, dest: String) -> Result<Vec<TrashEntry>, String> {
    list_trash_in(&state, &dest).await
}

#[tauri::command]
pub async fn restore_trash_entry(
    state: tauri::State<'_, RcdState>,
    dest: String,
    trash_path: String,
    original_path: String,
) -> Result<(), String> {
    restore_entry_in(&state, &dest, &trash_path, &original_path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_fs_for_is_a_sibling_of_a_nested_local_destination() {
        assert_eq!(trash_fs_for("/home/utente/foto"), Some("/home/utente/.rclone-easy-trash-foto".to_string()));
    }

    #[test]
    fn trash_fs_for_is_a_sibling_of_a_root_level_local_destination() {
        assert_eq!(trash_fs_for("/foto"), Some("/.rclone-easy-trash-foto".to_string()));
    }

    #[test]
    fn trash_fs_for_is_none_for_the_local_filesystem_root() {
        assert_eq!(trash_fs_for("/"), None);
    }

    #[test]
    fn trash_fs_for_is_a_sibling_of_a_nested_remote_destination() {
        assert_eq!(trash_fs_for("cubbit:mybucket/foto"), Some("cubbit:mybucket/.rclone-easy-trash-foto".to_string()));
    }

    #[test]
    fn trash_fs_for_is_a_sibling_of_a_root_level_remote_destination() {
        assert_eq!(trash_fs_for("cubbit:foto"), Some("cubbit:.rclone-easy-trash-foto".to_string()));
    }

    #[test]
    fn trash_fs_for_is_none_for_the_bare_remote_root() {
        assert_eq!(trash_fs_for("cubbit:"), None);
        assert_eq!(trash_fs_for("cubbit:/"), None);
    }

    #[test]
    fn parse_trashed_name_recovers_the_original_name_and_timestamp_with_extension() {
        assert_eq!(parse_trashed_name("foto.rce-trash-1755765000.jpg"), Some(("foto.jpg".to_string(), 1755765000)));
    }

    #[test]
    fn parse_trashed_name_recovers_the_original_name_without_extension() {
        assert_eq!(parse_trashed_name("README.rce-trash-1755765000"), Some(("README".to_string(), 1755765000)));
    }

    #[test]
    fn parse_trashed_name_is_none_for_a_file_not_produced_by_this_app() {
        assert_eq!(parse_trashed_name("foto-normale.jpg"), None);
        assert_eq!(parse_trashed_name("qualcosa.rce-trash-non-numerico.jpg"), None);
    }
}
