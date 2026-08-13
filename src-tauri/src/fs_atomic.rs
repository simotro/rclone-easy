//! Scrittura atomica di file di configurazione: `std::fs::write` da sola
//! tronca il file esistente prima di scrivere il nuovo contenuto — se il
//! processo viene terminato a metà (crash, chiusura forzata, spegnimento
//! del sistema) esattamente in quella finestra, il file resta troncato o
//! vuoto. Per `mounts.toml`/`jobs.toml`/`bisync.toml`, i cui struct hanno
//! `#[serde(default)]` sulla lista delle voci, un file vuoto o troncato in
//! un punto che lo rende comunque TOML valido viene interpretato come
//! "zero voci configurate" — non un errore visibile, una perdita silenziosa
//! di configurazione. Il rischio concreto maggiore è il salvataggio finale
//! dello storico a fine run (l'ultimo `save_to_dir` prima di un'eventuale
//! chiusura del processo), non la creazione iniziale del job.
//!
//! Scrive prima su un file temporaneo nella stessa cartella (stesso
//! filesystem del file di destinazione, quindi la `rename` successiva è
//! atomica sia su Linux/macOS sia su Windows — `std::fs::rename` su Windows
//! sostituisce la destinazione esistente, non fallisce come una `MoveFileEx`
//! senza `MOVEFILE_REPLACE_EXISTING`), e lo sincronizza su disco (`sync_all`,
//! non solo nella cache dell'OS) prima di sostituire il file vero: a quel
//! punto o il file vecchio resta intatto (crash prima della rename, che è
//! l'ultimo passo) o il nuovo è scritto per intero (rename già avvenuta) —
//! mai una via di mezzo troncata.
use std::io::Write;
use std::path::Path;

pub(crate) fn write_atomically(path: &Path, content: &str) -> Result<(), String> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("percorso non valido per una scrittura atomica: '{}'", path.display()))?;
    let tmp_path = path.with_file_name(format!("{file_name}.tmp"));

    let mut file =
        std::fs::File::create(&tmp_path).map_err(|e| format!("impossibile creare '{}': {e}", tmp_path.display()))?;
    file.write_all(content.as_bytes()).map_err(|e| format!("impossibile scrivere '{}': {e}", tmp_path.display()))?;
    file.sync_all().map_err(|e| format!("impossibile sincronizzare su disco '{}': {e}", tmp_path.display()))?;
    drop(file);

    std::fs::rename(&tmp_path, path).map_err(|e| format!("impossibile sostituire '{}': {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;

    #[test]
    fn write_atomically_creates_a_new_file_with_the_given_content() {
        let dir = TempDir::new("fs-atomic-new");
        std::fs::create_dir_all(&dir.path).unwrap();
        let path = dir.path.join("prova.toml");

        write_atomically(&path, "contenuto = \"x\"\n").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "contenuto = \"x\"\n");
    }

    #[test]
    fn write_atomically_replaces_existing_content_and_leaves_no_tmp_file_behind() {
        let dir = TempDir::new("fs-atomic-replace");
        std::fs::create_dir_all(&dir.path).unwrap();
        let path = dir.path.join("prova.toml");
        std::fs::write(&path, "vecchio = \"contenuto\"\n").unwrap();

        write_atomically(&path, "nuovo = \"contenuto\"\n").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "nuovo = \"contenuto\"\n");
        assert!(!dir.path.join("prova.toml.tmp").exists(), "il file temporaneo deve sparire dopo la rename");
    }
}
