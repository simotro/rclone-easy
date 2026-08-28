use crate::rcd::{self, RcdState};
use serde::Serialize;

/// Cartella dedicata dove "Sposta per revisione" mette da parte un
/// duplicato specifico (per ID, univoco anche quando il nome è ambiguo) —
/// sullo stesso remote, mai altrove, così resta raggiungibile/recuperabile
/// da lì con calma. Esclusa dal confronto di bisync (vedi il suo utilizzo in
/// `bisync.rs`) per lo stesso motivo di `remote_lock::LOCK_FOLDER_EXCLUDE`:
/// altrimenti finirebbe trattata come contenuto vero da risincronizzare.
const REVIEW_FOLDER: &str = ".rclone-easy-duplicates-review";

pub(crate) const REVIEW_FOLDER_EXCLUDE: &str = "/.rclone-easy-duplicates-review/**";

/// Un singolo oggetto di un gruppo di nomi duplicati sullo stesso remote —
/// vedi `bisync::extract_duplicate_names` per come viene scoperto il nome in
/// conflitto. `fs` è la radice del remote su cui vive davvero (path1 o path2
/// del job bisync, un nome duplicato può trovarsi su uno dei due) — serve a
/// `move_for_review_in`/`delete_in` per sapere su quale fs operare.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateObject {
    pub id: String,
    pub size: i64,
    pub mod_time: String,
    pub fs: String,
}

/// `(prefisso del remote incluso ":", percorso senza lo slash finale)` —
/// stessa logica di `trash::split_fs`, duplicata qui apposta (vedi il
/// commento su quella funzione sul perché non è condivisa tra i moduli).
fn split_remote_and_path(fs: &str) -> (String, String) {
    if std::path::Path::new(fs).is_absolute() {
        (String::new(), fs.trim_end_matches('/').to_string())
    } else if let Some((name, rest)) = fs.split_once(':') {
        (format!("{name}:"), rest.trim_end_matches('/').to_string())
    } else {
        (String::new(), fs.trim_end_matches('/').to_string())
    }
}

/// Combina la radice di un lato del job (`root`, es. "remote-a:sotto"
/// o un percorso locale) con il nome trovato nel log
/// (`name`, può includere sue proprie sottocartelle) in un percorso
/// completo relativo al remote — usato sia per listare la cartella padre
/// (serve il suo genitore, non il file stesso: `operations/list` non ha un
/// filtro "solo questo nome") sia per calcolare la nuova destinazione di
/// `moveid`, sempre relativa alla stessa radice.
fn resolve_relative_path(root: &str, name: &str) -> (String, String) {
    let (remote_prefix, root_path) = split_remote_and_path(root);
    let combined = if root_path.is_empty() {
        name.trim_start_matches('/').to_string()
    } else {
        format!("{root_path}/{}", name.trim_start_matches('/'))
    };
    (remote_prefix, combined)
}

/// `root` è `path1` o `path2` del job così come configurato (può includere
/// una sottocartella, es. "remote-a:Squadra/Documenti") — finisce
/// direttamente in `DuplicateObject.fs`, che deve restare sulla STESSA base
/// usata per interpretare `name` in `move_for_review_in`/`delete_in`
/// (entrambi relativi a `root`, mai al bare `remote_prefix`): altrimenti
/// `moveid` riceverebbe un `fs` diverso dalla cartella dove l'oggetto vive
/// davvero, tentando di creare `REVIEW_FOLDER` nella radice nuda del
/// remote invece che nella sottocartella configurata — dove l'account può
/// non avere permesso di scrittura anche quando ce l'ha nella sottocartella
/// (bug reale: `mkdir .rclone-easy-duplicates-review: read-only file
/// system` su un remote con `path1`/`path2` puntato a una sottocartella).
async fn list_matches_on(info: &rcd::ConnectionInfo, root: &str, remote_prefix: &str, combined_path: &str) -> Vec<DuplicateObject> {
    let basename = combined_path.rsplit('/').next().unwrap_or(combined_path);
    let parent = &combined_path[..combined_path.len() - basename.len()];
    let parent = parent.trim_end_matches('/');
    let list_fs = format!("{remote_prefix}{parent}");

    let Ok(body) = info.call("operations/list", serde_json::json!({ "fs": list_fs, "remote": "", "opt": { "filesOnly": true, "noModTime": false } })).await else {
        return Vec::new();
    };
    let Some(list) = body.get("list").and_then(|l| l.as_array()) else { return Vec::new() };

    list.iter()
        .filter(|item| item.get("Name").and_then(|n| n.as_str()) == Some(basename))
        .filter_map(|item| {
            Some(DuplicateObject {
                id: item.get("ID")?.as_str()?.to_string(),
                size: item.get("Size").and_then(|s| s.as_i64()).unwrap_or(-1),
                mod_time: item.get("ModTime").and_then(|m| m.as_str()).unwrap_or_default().to_string(),
                fs: root.to_string(),
            })
        })
        .collect()
}

/// Trova tutti gli oggetti che condividono `name` nella stessa cartella, su
/// uno dei due lati del job (`path1`/`path2`) — un nome duplicato può
/// trovarsi solo sul lato che lo permette per costruzione (in pratica solo
/// un remote come Google Drive, mai un percorso locale né un backend che
/// impone nomi univoci), ma questa funzione non presume quale dei due sia:
/// prova entrambi, restituisce il primo lato con più di un oggetto trovato.
pub(crate) async fn list_group_in(state: &RcdState, path1: &str, path2: &str, name: &str) -> Result<Vec<DuplicateObject>, String> {
    let info = rcd::connection_info(state).await?;
    for root in [path1, path2] {
        let (remote_prefix, combined) = resolve_relative_path(root, name);
        if remote_prefix.is_empty() {
            // Un percorso locale non può avere nomi duplicati nella stessa
            // cartella per costruzione (il filesystem stesso lo impedisce) —
            // salta senza nemmeno interrogare rcd.
            continue;
        }
        let matches = list_matches_on(&info, root, &remote_prefix, &combined).await;
        if matches.len() > 1 {
            return Ok(matches);
        }
    }
    Ok(Vec::new())
}

#[tauri::command]
pub async fn list_duplicate_group(state: tauri::State<'_, RcdState>, path1: String, path2: String, name: String) -> Result<Vec<DuplicateObject>, String> {
    list_group_in(&state, &path1, &path2, &name).await
}

/// Percorso di destinazione univoco dentro `REVIEW_FOLDER` per un oggetto —
/// l'ID va subito prima del nome del file (non del percorso intero) così la
/// struttura delle sottocartelle originali resta leggibile, e l'unicità è
/// garantita indipendentemente da cosa c'è già nella cartella di revisione.
fn review_destination(id: &str, combined_path: &str) -> String {
    let basename = combined_path.rsplit('/').next().unwrap_or(combined_path);
    let dir = &combined_path[..combined_path.len() - basename.len()];
    format!("{REVIEW_FOLDER}/{dir}{id}__{basename}")
}

async fn moveid(info: &rcd::ConnectionInfo, fs: &str, id: &str, dest_path: &str) -> Result<(), String> {
    info.call("backend/command", serde_json::json!({ "command": "moveid", "fs": fs, "arg": [id, dest_path] })).await?;
    Ok(())
}

/// Sposta un oggetto specifico (per ID, senza ambiguità di nome) in
/// `REVIEW_FOLDER` sullo stesso remote — non cancella nulla, risolve solo
/// la collisione di nome nella cartella originale: dal giro successivo
/// bisync non lo segnala più come duplicato lì, e l'oggetto resta
/// recuperabile finché non si decide cosa farne con calma. `fs` (radice
/// completa, vedi `list_matches_on`) e `name` sono già sulla stessa base —
/// niente `resolve_relative_path` qui, servirebbe solo a chi parte da un
/// nome relativo a un'altra radice.
pub(crate) async fn move_for_review_in(state: &RcdState, fs: &str, id: &str, name: &str) -> Result<(), String> {
    let info = rcd::connection_info(state).await?;
    let dest = review_destination(id, name.trim_start_matches('/'));
    moveid(&info, fs, id, &dest).await
}

#[tauri::command]
pub async fn move_duplicate_for_review(state: tauri::State<'_, RcdState>, fs: String, id: String, name: String) -> Result<(), String> {
    move_for_review_in(&state, &fs, &id, &name).await
}

/// Elimina un oggetto specifico (per ID). In due passi — prima uno
/// spostamento su un percorso univoco (stessa funzione di "Sposta per
/// revisione"), poi la cancellazione da lì — mai una cancellazione diretta
/// per nome, che sull'oggetto sbagliato del gruppo sarebbe indistinguibile
/// da quella giusta. Il remote resta comunque quello che decide se
/// l'eliminazione è recuperabile (`use_trash`, di norma attivo per Google
/// Drive: finisce nel Cestino nativo del servizio, non sparisce all'istante).
pub(crate) async fn delete_in(state: &RcdState, fs: &str, id: &str, name: &str) -> Result<(), String> {
    let info = rcd::connection_info(state).await?;
    let dest = review_destination(id, name.trim_start_matches('/'));
    moveid(&info, fs, id, &dest).await?;
    info.call("operations/deletefile", serde_json::json!({ "fs": fs, "remote": dest })).await?;
    Ok(())
}

#[tauri::command]
pub async fn delete_duplicate(state: tauri::State<'_, RcdState>, fs: String, id: String, name: String) -> Result<(), String> {
    delete_in(&state, &fs, &id, &name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_relative_path_combines_a_remote_root_with_a_nested_name() {
        let (prefix, combined) = resolve_relative_path("remote-a:sotto", "cartella/Elenco (2).xlsx");
        assert_eq!(prefix, "remote-a:");
        assert_eq!(combined, "sotto/cartella/Elenco (2).xlsx");
    }

    #[test]
    fn resolve_relative_path_handles_a_bare_remote_root() {
        let (prefix, combined) = resolve_relative_path("remote-b:", "doc.gform");
        assert_eq!(prefix, "remote-b:");
        assert_eq!(combined, "doc.gform");
    }

    #[test]
    fn resolve_relative_path_returns_an_empty_prefix_for_a_local_path() {
        let (prefix, combined) = resolve_relative_path("/home/simone/Drive Copass", "doc.odt");
        assert_eq!(prefix, "");
        assert_eq!(combined, "/home/simone/Drive Copass/doc.odt");
    }

    #[test]
    fn review_destination_keeps_the_original_folder_structure_readable() {
        let dest = review_destination("1abc", "sotto/cartella/Elenco (2).xlsx");
        assert_eq!(dest, ".rclone-easy-duplicates-review/sotto/cartella/1abc__Elenco (2).xlsx");
    }

    #[test]
    fn review_destination_works_for_a_name_without_subfolders() {
        let dest = review_destination("1abc", "doc.gform");
        assert_eq!(dest, ".rclone-easy-duplicates-review/1abc__doc.gform");
    }

    /// Un nome duplicato nella stessa cartella può esistere solo su un
    /// backend che lo permette per costruzione (in pratica solo Google
    /// Drive tra quelli comuni) — non riproducibile con una coppia di
    /// cartelle locali reali, dato che il filesystem stesso lo impedisce.
    /// Questo test copre solo quello che UN percorso locale reale può
    /// verificare: nessun errore, nessuna chiamata rcd sprecata sul lato
    /// locale, risultato vuoto quando non c'è nulla da trovare su nessuno
    /// dei due lati.
    #[tokio::test]
    async fn list_group_in_skips_local_sides_and_returns_empty_when_nothing_matches() {
        use crate::rcd::tests::TempDir;

        let config_dir = TempDir::new("duplicates-list-config");
        let path1_dir = TempDir::new("duplicates-list-path1");
        let path2_dir = TempDir::new("duplicates-list-path2");
        std::fs::create_dir_all(&path1_dir.path).unwrap();
        std::fs::create_dir_all(&path2_dir.path).unwrap();
        std::fs::write(path1_dir.path.join("doc.txt"), "x").unwrap();

        let state = crate::rcd::build_state(config_dir.config_path()).await;
        let result = list_group_in(&state, &path1_dir.path.to_string_lossy(), &path2_dir.path.to_string_lossy(), "doc.txt").await;

        assert_eq!(result, Ok(Vec::new()), "due percorsi locali non possono avere nomi duplicati: {result:?}");
    }
}
