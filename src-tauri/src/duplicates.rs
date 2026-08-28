use crate::rcd::{self, RcdState};
use serde::Serialize;
use std::path::Path;

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
    if Path::new(fs).is_absolute() {
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

/// Antepone `fs` a un percorso relativo per ottenere un percorso rclone
/// completo ("remote:sotto/percorso") — necessario per `moveid`: la
/// documentazione di rclone lo descrive genericamente come "an rclone
/// path", e non è relativo a `fs` come si potrebbe presumere guardando gli
/// altri comandi `backend/command` di questo modulo. Verificato dal vivo:
/// passandogli un percorso senza `remote:` davanti, rclone lo tratta come
/// un percorso LOCALE relativo alla working directory del processo `rclone
/// rcd` (che nel bundle AppImage è dentro il punto di mount squashfs, di
/// sola lettura — da cui il fuorviante "mkdir ...: read-only file system",
/// un errore del filesystem locale, non del remote). Senza questa
/// qualificazione esplicita, "Sposta per revisione"/"Elimina" scaricavano
/// l'oggetto in una cartella locale e cancellavano l'originale dal remote,
/// invece di spostarlo lì.
fn qualify(fs: &str, relative: &str) -> String {
    if fs.ends_with(':') {
        format!("{fs}{relative}")
    } else {
        format!("{fs}/{relative}")
    }
}

async fn moveid(info: &rcd::ConnectionInfo, fs: &str, id: &str, dest_path: &str) -> Result<(), String> {
    info.call("backend/command", serde_json::json!({ "command": "moveid", "fs": fs, "arg": [id, qualify(fs, dest_path)] })).await?;
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
/// Spostamento e cancellazione in rapida sequenza tramite `rcd` (non un
/// sottoprocesso a sé come `delete_review_entry_in`): verificato dal vivo
/// più volte che questa sequenza specifica riesce sempre — il problema
/// osservato con "object not found" (vedi `delete_review_entry_in`) si
/// manifesta solo su un oggetto rimasto per un po' nella cartella di
/// revisione, mai su uno appena spostato lì.
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

/// Apre nel file manager/app predefinita il file locale che corrisponde per
/// nome a un oggetto duplicato o a una voce in attesa di revisione — un
/// job bisync ha sempre un solo lato locale, e il nome (ricostruito dal log
/// o dalla cartella di revisione) è sempre relativo a quella stessa radice.
/// Best-effort come `mounts::open_mount_folder`: non verifica che il file
/// esista davvero (potrebbe non esserci ancora, o essere quello dell'ALTRO
/// duplicato con lo stesso nome — non c'è modo di saperlo per costruzione),
/// apre comunque il percorso e lascia che sia l'app di sistema a segnalare
/// un eventuale file mancante.
#[tauri::command]
pub fn open_local_duplicate(local_root: String, name: String) {
    let path = Path::new(&local_root).join(name.trim_start_matches('/'));
    crate::open_external::open_path(&path.to_string_lossy());
}

/// Un oggetto già spostato in `REVIEW_FOLDER` — a differenza di
/// `DuplicateObject`, `name` è già il percorso completo ricostruito
/// (struttura di sottocartelle originale inclusa, vedi `review_destination`),
/// pronto da mostrare in UI senza ulteriori calcoli.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewEntry {
    pub id: String,
    pub name: String,
    pub size: i64,
    pub mod_time: String,
    /// Radice del remote su cui vive (per `delete_review_entry`/apertura
    /// remota) — stesso principio di `DuplicateObject::fs`.
    pub fs: String,
    /// Percorso relativo dentro `fs` (comprensivo del prefisso
    /// `REVIEW_FOLDER`), da ripassare a `delete_review_entry`.
    pub review_path: String,
}

fn extract_list(body: &serde_json::Value) -> Result<Vec<serde_json::Value>, String> {
    body.get("list").and_then(|v| v.as_array()).cloned().ok_or_else(|| format!("campo 'list' mancante nella risposta di rclone rcd: {body}"))
}

/// Toglie il prefisso `"{id}__"` da un nome dentro `REVIEW_FOLDER` (vedi
/// `review_destination`) per recuperare il nome originale. `id` va preso
/// dal campo `ID` della risposta di `operations/list` (l'ID vero e attuale
/// dell'oggetto), MAI ricavato tagliando la stringa alla prima/ultima `__`
/// trovata: un ID Drive reale può a sua volta terminare con `_` (osservato
/// dal vivo, es. `"...LlV3u_"`), che sommato al separatore produce tre
/// underscore consecutivi — tagliare al primo `__` in quel caso perde
/// l'ultimo carattere dell'ID vero. Conoscendo già l'ID per altra via, un
/// semplice `strip_prefix` è inequivocabile a prescindere da cosa
/// contengano ID o nome originale.
fn parse_review_name(id: &str, name: &str) -> Option<String> {
    name.strip_prefix(&format!("{id}__")).map(str::to_string)
}

/// `listing_path` è il campo `Path` di una voce elencata con `fs` già dentro
/// `REVIEW_FOLDER` (vedi `list_review_entries_in`, che lista
/// `qualify(root, REVIEW_FOLDER)`) — quindi NON contiene il prefisso di
/// `REVIEW_FOLDER`. `ReviewEntry.fs`, però, è la radice NUDA (`root`, senza
/// `REVIEW_FOLDER`): per restare sulla stessa base quando i due campi
/// vengono ricombinati altrove (`delete_review_entry_in`), va riaggiunto
/// qui il prefisso mancante.
fn review_path_relative_to_fs(listing_path: &str) -> String {
    format!("{REVIEW_FOLDER}/{listing_path}")
}

/// Elenca il contenuto di `REVIEW_FOLDER` su entrambi i lati del job (un
/// nome duplicato può essere stato trovato su uno dei due, vedi
/// `list_group_in`) — lista vuota, non un errore, se quel lato è locale o se
/// la cartella di revisione non esiste ancora lì.
pub(crate) async fn list_review_entries_in(state: &RcdState, path1: &str, path2: &str) -> Result<Vec<ReviewEntry>, String> {
    let info = rcd::connection_info(state).await?;
    let mut entries = Vec::new();
    for root in [path1, path2] {
        let (remote_prefix, _) = split_remote_and_path(root);
        if remote_prefix.is_empty() {
            continue;
        }
        let list_fs = qualify(root, REVIEW_FOLDER);
        let body = info
            .call("operations/list", serde_json::json!({ "fs": list_fs, "remote": "", "opt": { "filesOnly": true, "recurse": true, "noModTime": false } }))
            .await;
        let body = match body {
            Ok(body) => body,
            Err(e) if e.contains("directory not found") => continue,
            Err(e) => return Err(e),
        };
        for item in extract_list(&body)? {
            let path = item.get("Path").and_then(|v| v.as_str()).unwrap_or_default();
            let basename = item.get("Name").and_then(|v| v.as_str()).unwrap_or_default();
            let Some(id) = item.get("ID").and_then(|v| v.as_str()) else { continue };
            let Some(original_name) = parse_review_name(id, basename) else { continue };
            let dir_prefix = path.strip_suffix(basename).unwrap_or("");
            entries.push(ReviewEntry {
                id: id.to_string(),
                name: format!("{dir_prefix}{original_name}"),
                size: item.get("Size").and_then(|v| v.as_i64()).unwrap_or(-1),
                mod_time: item.get("ModTime").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                fs: root.to_string(),
                // `path` è relativo a `list_fs` (già dentro REVIEW_FOLDER),
                // ma `fs` sopra è la radice NUDA (`root`) — per restare sulla
                // stessa base dei due campi insieme (necessario a
                // `delete_review_entry_in`, che li combina), va riportato
                // relativo a `fs` riaggiungendo il prefisso di REVIEW_FOLDER.
                // Bug reale trovato dal vivo: senza questo, "Elimina
                // definitivamente" costruiva un percorso che puntava
                // all'ALBERO VERO (fuori da REVIEW_FOLDER) invece che dentro
                // la cartella di revisione, fallendo con "object not found"
                // in modo del tutto deterministico — non un problema di
                // demone/cache come inizialmente sospettato.
                review_path: review_path_relative_to_fs(path),
            });
        }
    }
    entries.sort_by(|a, b| b.mod_time.cmp(&a.mod_time));
    Ok(entries)
}

#[tauri::command]
pub async fn list_review_entries(state: tauri::State<'_, RcdState>, path1: String, path2: String) -> Result<Vec<ReviewEntry>, String> {
    list_review_entries_in(&state, &path1, &path2).await
}

/// Cancella per sempre una voce già in `REVIEW_FOLDER` — a differenza di
/// `delete_in`, nessuno spostamento preliminare: il nome lì dentro è già
/// univoco (prefisso ID), niente ambiguità da risolvere. `review_path` è
/// già relativo a `fs` (vedi il commento su `ReviewEntry::review_path`).
pub(crate) async fn delete_review_entry_in(state: &RcdState, fs: &str, review_path: &str) -> Result<(), String> {
    let info = rcd::connection_info(state).await?;
    info.call("operations/deletefile", serde_json::json!({ "fs": fs, "remote": review_path })).await?;
    Ok(())
}

#[tauri::command]
pub async fn delete_review_entry(state: tauri::State<'_, RcdState>, fs: String, review_path: String) -> Result<(), String> {
    delete_review_entry_in(&state, &fs, &review_path).await
}

/// Inserisce un suffisso numerico progressivo prima dell'estensione (o in
/// fondo al nome se non ne ha una) — stessa convenzione "nome (1).ext" già
/// familiare da file manager/browser per "mantieni entrambi".
fn suffixed_name(name: &str, index: usize) -> String {
    let basename = name.rsplit('/').next().unwrap_or(name);
    let dir = &name[..name.len() - basename.len()];
    match basename.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{dir}{stem} ({index}).{ext}"),
        _ => format!("{dir}{basename} ({index})"),
    }
}

/// Nomi (basename, non percorso intero) già presenti nella cartella padre di
/// `name` su `root` — usata da `keep_all_in` per non scegliere un suffisso
/// che collide con qualcos'altro già lì.
async fn existing_names_in_parent(info: &rcd::ConnectionInfo, root: &str, name: &str) -> Vec<String> {
    let (remote_prefix, combined) = resolve_relative_path(root, name);
    let basename = combined.rsplit('/').next().unwrap_or(&combined);
    let parent = &combined[..combined.len() - basename.len()];
    let parent = parent.trim_end_matches('/');
    let list_fs = format!("{remote_prefix}{parent}");
    let Ok(body) = info.call("operations/list", serde_json::json!({ "fs": list_fs, "remote": "", "opt": { "filesOnly": true, "noModTime": true } })).await else {
        return Vec::new();
    };
    body.get("list")
        .and_then(|l| l.as_array())
        .map(|list| list.iter().filter_map(|item| item.get("Name").and_then(|n| n.as_str()).map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Risolve un intero gruppo di duplicati tenendoli TUTTI, invece di
/// scegliere quale sacrificare: il più vecchio (per `ModTime`) resta col
/// nome originale, gli altri vengono rinominati sul posto (stessa cartella,
/// solo il nome cambia — mai spostati altrove) con un suffisso numerico
/// progressivo, saltando eventuali nomi già occupati. Da quel momento non
/// c'è più ambiguità di nome: bisync smette di segnalarli come duplicati
/// dal giro successivo.
pub(crate) async fn keep_all_in(state: &RcdState, path1: &str, path2: &str, name: &str) -> Result<(), String> {
    let mut objects = list_group_in(state, path1, path2, name).await?;
    if objects.len() < 2 {
        return Ok(());
    }
    objects.sort_by(|a, b| a.mod_time.cmp(&b.mod_time));

    let info = rcd::connection_info(state).await?;
    let root = objects[0].fs.clone();
    let mut used: std::collections::HashSet<String> = existing_names_in_parent(&info, &root, name).await.into_iter().collect();

    let mut next_index = 1usize;
    for obj in objects.iter().skip(1) {
        let mut candidate = suffixed_name(name, next_index);
        let mut candidate_basename = candidate.rsplit('/').next().unwrap_or(&candidate).to_string();
        while used.contains(&candidate_basename) {
            next_index += 1;
            candidate = suffixed_name(name, next_index);
            candidate_basename = candidate.rsplit('/').next().unwrap_or(&candidate).to_string();
        }
        used.insert(candidate_basename);
        next_index += 1;
        moveid(&info, &obj.fs, &obj.id, &candidate).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn keep_all_duplicates(state: tauri::State<'_, RcdState>, path1: String, path2: String, name: String) -> Result<(), String> {
    keep_all_in(&state, &path1, &path2, &name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixed_name_inserts_the_index_before_the_extension() {
        assert_eq!(suffixed_name("Elenco.xlsx", 1), "Elenco (1).xlsx");
    }

    #[test]
    fn suffixed_name_appends_the_index_when_there_is_no_extension() {
        assert_eq!(suffixed_name("README", 2), "README (2)");
    }

    #[test]
    fn suffixed_name_preserves_a_subfolder_prefix() {
        assert_eq!(suffixed_name("cartella/Elenco (2).xlsx", 1), "cartella/Elenco (2) (1).xlsx");
    }

    #[test]
    fn suffixed_name_treats_a_leading_dot_as_no_extension() {
        // Es. ".gitignore": rsplit_once('.') darebbe stem vuoto — trattato
        // come "senza estensione" invece di produrre " (1).gitignore".
        assert_eq!(suffixed_name(".gitignore", 1), ".gitignore (1)");
    }

    #[test]
    fn qualify_joins_a_bare_remote_root_without_an_extra_slash() {
        assert_eq!(qualify("remote-a:", ".rclone-easy-duplicates-review/1abc__doc.xlsx"), "remote-a:.rclone-easy-duplicates-review/1abc__doc.xlsx");
    }

    #[test]
    fn qualify_joins_a_remote_root_with_a_subfolder() {
        assert_eq!(
            qualify("remote-a:sotto/cartella", ".rclone-easy-duplicates-review/1abc__doc.xlsx"),
            "remote-a:sotto/cartella/.rclone-easy-duplicates-review/1abc__doc.xlsx"
        );
    }

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

    /// Bug reale trovato dal vivo: senza il prefisso di `REVIEW_FOLDER`
    /// riaggiunto qui, "Elimina definitivamente" costruiva un percorso che
    /// puntava all'albero vero (fuori dalla cartella di revisione) invece
    /// che dentro, fallendo sempre con "object not found" — non un
    /// problema di demone/cache come inizialmente sospettato.
    #[test]
    fn review_path_relative_to_fs_reattaches_the_review_folder_prefix() {
        assert_eq!(
            review_path_relative_to_fs("PRESENZE e PAGHE/BONUS NATALE 2024/1abc__nota.pdf"),
            ".rclone-easy-duplicates-review/PRESENZE e PAGHE/BONUS NATALE 2024/1abc__nota.pdf"
        );
    }

    #[test]
    fn parse_review_name_strips_the_known_id_prefix() {
        assert_eq!(parse_review_name("1abc", "1abc__Elenco (2).xlsx"), Some("Elenco (2).xlsx".to_string()));
    }

    #[test]
    fn parse_review_name_is_none_when_the_name_does_not_start_with_that_id() {
        assert_eq!(parse_review_name("1abc", "documento-normale.xlsx"), None);
    }

    /// Caso reale osservato dal vivo: un ID Drive può terminare con `_`,
    /// producendo tre underscore consecutivi prima del nome originale
    /// (uno dell'ID, due del separatore) — un taglio "al primo/ultimo `__`"
    /// romperebbe questo caso, `strip_prefix` con l'ID già noto no.
    #[test]
    fn parse_review_name_handles_an_id_that_itself_ends_with_an_underscore() {
        assert_eq!(
            parse_review_name("1emtJL_Q-tuPrCmbbjjbnL-ICM5LlV3u_", "1emtJL_Q-tuPrCmbbjjbnL-ICM5LlV3u___nota.docx"),
            Some("nota.docx".to_string())
        );
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

    /// Stessa cautela di `list_group_in_skips_local_sides...`: una cartella
    /// di revisione può esistere solo sul lato remoto, mai su quello locale.
    #[tokio::test]
    async fn list_review_entries_in_skips_local_sides_and_returns_empty_when_nothing_matches() {
        use crate::rcd::tests::TempDir;

        let config_dir = TempDir::new("duplicates-review-list-config");
        let path1_dir = TempDir::new("duplicates-review-list-path1");
        let path2_dir = TempDir::new("duplicates-review-list-path2");
        std::fs::create_dir_all(&path1_dir.path).unwrap();
        std::fs::create_dir_all(&path2_dir.path).unwrap();

        let state = crate::rcd::build_state(config_dir.config_path()).await;
        let result = list_review_entries_in(&state, &path1_dir.path.to_string_lossy(), &path2_dir.path.to_string_lossy()).await;

        assert_eq!(result, Ok(Vec::new()), "due percorsi locali non possono avere una cartella di revisione: {result:?}");
    }
}
