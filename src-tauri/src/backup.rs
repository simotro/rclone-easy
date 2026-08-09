use age::secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// Formato di export/import della config propria dell'app (non del
/// `rclone.conf` di sistema, mai toccato) — un unico file JSON che
/// racchiude i quattro file sotto `app_config_dir()`, così l'utente ha un
/// solo file da salvare/spostare invece di una cartella intera. `None` per
/// un campo significa "quel file non esisteva quando è stato fatto il
/// backup" (es. nessun mount configurato), non un errore.
#[derive(Debug, Serialize, Deserialize)]
struct BackupBundle {
    format_version: u32,
    rclone_conf: Option<String>,
    mounts_toml: Option<String>,
    jobs_toml: Option<String>,
    bisync_toml: Option<String>,
}

fn read_optional(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn build_backup(config_dir: &Path) -> BackupBundle {
    BackupBundle {
        format_version: 1,
        rclone_conf: read_optional(&config_dir.join("rclone.conf")),
        mounts_toml: read_optional(&config_dir.join("mounts.toml")),
        jobs_toml: read_optional(&config_dir.join("jobs.toml")),
        bisync_toml: read_optional(&config_dir.join("bisync.toml")),
    }
}

pub(crate) fn create_backup_in(config_dir: &Path) -> Result<String, String> {
    serde_json::to_string_pretty(&build_backup(config_dir)).map_err(|e| format!("impossibile creare il backup: {e}"))
}

/// Cifra il JSON del backup con una passphrase (formato age, cifratura
/// scrypt) — il bundle contiene `rclone.conf`, le cui credenziali rclone
/// "offusca" con un XOR banale reversibile da chiunque conosca
/// l'algoritmo, non una cifratura vera. Il file su disco risultante è
/// binario puro (nessun involucro JSON attorno): a differenza del vecchio
/// formato non è più leggibile/ispezionabile senza la password.
fn encrypt_bundle(bundle_json: &str, password: &str) -> Result<Vec<u8>, String> {
    let encryptor = age::Encryptor::with_user_passphrase(SecretString::from(password.to_string()));
    let mut encrypted = Vec::new();
    let mut writer = encryptor.wrap_output(&mut encrypted).map_err(|e| format!("impossibile cifrare il backup: {e}"))?;
    writer.write_all(bundle_json.as_bytes()).map_err(|e| format!("impossibile cifrare il backup: {e}"))?;
    writer.finish().map_err(|e| format!("impossibile cifrare il backup: {e}"))?;
    Ok(encrypted)
}

/// Decifra un backup prodotto da `encrypt_bundle`. Un errore qui copre sia
/// password sbagliata sia file corrotto — indistinguibili per costruzione
/// in una cifratura autenticata (age verifica un MAC, non "prova" chiavi
/// diverse), quindi il messaggio resta volutamente generico.
fn decrypt_bundle(encrypted: &[u8], password: &str) -> Result<String, String> {
    let identity = age::scrypt::Identity::new(SecretString::from(password.to_string()));
    let mut decrypted = Vec::new();
    let mut reader = age::Decryptor::new(encrypted)
        .map_err(|_| "file di backup non valido o corrotto".to_string())?
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|_| "password errata o file di backup corrotto".to_string())?;
    reader.read_to_end(&mut decrypted).map_err(|_| "password errata o file di backup corrotto".to_string())?;
    String::from_utf8(decrypted).map_err(|e| format!("backup corrotto: {e}"))
}

/// Scrive un file, sovrascrivendo se già esistente, senza fallire se il
/// contenuto è assenzente (`None` restituito da `build_backup` per un file
/// che non esisteva al momento del backup): scrivere una stringa vuota è
/// equivalente a "nessuna voce" per tutti e quattro i file (TOML vuoto =
/// tutti i campi ai valori di default, `rclone.conf` vuoto = nessun
/// remote), stesso comportamento di quando il file manca del tutto.
fn write_field(path: &Path, content: Option<&str>) -> Result<(), String> {
    std::fs::write(path, content.unwrap_or("")).map_err(|e| format!("impossibile scrivere '{}': {e}", path.display()))
}

pub(crate) fn restore_backup_in(config_dir: &Path, bundle_json: &str) -> Result<(), String> {
    let bundle: BackupBundle =
        serde_json::from_str(bundle_json).map_err(|e| format!("file di backup non valido o corrotto: {e}"))?;

    std::fs::create_dir_all(config_dir)
        .map_err(|e| format!("impossibile creare '{}': {e}", config_dir.display()))?;

    write_field(&config_dir.join("rclone.conf"), bundle.rclone_conf.as_deref())?;
    write_field(&config_dir.join("mounts.toml"), bundle.mounts_toml.as_deref())?;
    write_field(&config_dir.join("jobs.toml"), bundle.jobs_toml.as_deref())?;
    write_field(&config_dir.join("bisync.toml"), bundle.bisync_toml.as_deref())?;
    Ok(())
}

fn app_config_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))
}

/// Salva il backup, cifrato con la password scelta dall'utente, nel file
/// scelto dal selettore "Salva" nel frontend. Non richiede che il demone
/// rcd sia coinvolto: legge solo file già su disco. La password è
/// obbligatoria: `rclone.conf` contiene credenziali solo "offuscate" da
/// rclone (reversibili, non una cifratura vera), quindi il file di backup
/// non deve mai finire su disco leggibile in chiaro.
#[tauri::command]
pub async fn export_backup(app: AppHandle, destination_path: String, password: String) -> Result<(), String> {
    let bundle_json = create_backup_in(&app_config_dir(&app)?)?;
    let encrypted = encrypt_bundle(&bundle_json, &password)?;
    std::fs::write(&destination_path, encrypted).map_err(|e| format!("impossibile scrivere '{destination_path}': {e}"))
}

/// Ripristina la config propria dell'app da un file scelto dall'utente,
/// sovrascrivendo quella attuale — distruttivo, il frontend deve aver già
/// ottenuto conferma esplicita prima di chiamarlo. Non riavvia da solo il
/// demone rcd né i mount/job già in corso: quelli restano con lo stato
/// precedente finché l'utente non riavvia l'app, il frontend deve avvisarlo.
///
/// `password` assente o vuota: il file deve essere un backup in chiaro di
/// una versione precedente dell'app (da prima che l'export cifrasse
/// sempre) — se invece è cifrato, l'errore lo dice esplicitamente così il
/// frontend può far inserire una password e ritentare. `password` presente:
/// il file viene sempre decifrato con quella, mai letto come testo in
/// chiaro anche se per qualche motivo lo fosse ancora.
#[tauri::command]
pub async fn import_backup(app: AppHandle, source_path: String, password: Option<String>) -> Result<(), String> {
    let bytes = std::fs::read(&source_path).map_err(|e| format!("impossibile leggere '{source_path}': {e}"))?;
    let bundle_json = match password.filter(|p| !p.is_empty()) {
        Some(password) => decrypt_bundle(&bytes, &password)?,
        None => String::from_utf8(bytes)
            .map_err(|_| "questo backup è cifrato: inserisci la password per ripristinarlo".to_string())?,
    };
    restore_backup_in(&app_config_dir(&app)?, &bundle_json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;

    #[test]
    fn create_backup_in_reads_the_four_files_that_exist() {
        let dir = TempDir::new("backup-create");
        std::fs::create_dir_all(&dir.path).unwrap();
        std::fs::write(dir.path.join("rclone.conf"), "[cubbit]\ntype = s3\n").unwrap();
        std::fs::write(dir.path.join("mounts.toml"), "[[mounts]]\nname = \"x\"\n").unwrap();
        // jobs.toml e bisync.toml assenti apposta: un backup deve comunque riuscire.

        let json = create_backup_in(&dir.path).unwrap();
        let bundle: BackupBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(bundle.rclone_conf.as_deref(), Some("[cubbit]\ntype = s3\n"));
        assert_eq!(bundle.mounts_toml.as_deref(), Some("[[mounts]]\nname = \"x\"\n"));
        assert_eq!(bundle.jobs_toml, None);
        assert_eq!(bundle.bisync_toml, None);
    }

    #[test]
    fn restore_backup_in_writes_back_all_four_files() {
        let dir = TempDir::new("backup-restore");
        let json = serde_json::to_string(&BackupBundle {
            format_version: 1,
            rclone_conf: Some("[cubbit]\ntype = s3\n".to_string()),
            mounts_toml: Some("[[mounts]]\nname = \"x\"\n".to_string()),
            jobs_toml: None,
            bisync_toml: None,
        })
        .unwrap();

        restore_backup_in(&dir.path, &json).unwrap();

        assert_eq!(std::fs::read_to_string(dir.path.join("rclone.conf")).unwrap(), "[cubbit]\ntype = s3\n");
        assert_eq!(std::fs::read_to_string(dir.path.join("mounts.toml")).unwrap(), "[[mounts]]\nname = \"x\"\n");
        assert_eq!(std::fs::read_to_string(dir.path.join("jobs.toml")).unwrap(), "");
        assert_eq!(std::fs::read_to_string(dir.path.join("bisync.toml")).unwrap(), "");
    }

    #[test]
    fn restore_backup_in_overwrites_existing_files() {
        let dir = TempDir::new("backup-restore-overwrite");
        std::fs::create_dir_all(&dir.path).unwrap();
        std::fs::write(dir.path.join("jobs.toml"), "vecchio contenuto da sovrascrivere").unwrap();

        let json = serde_json::to_string(&BackupBundle {
            format_version: 1,
            rclone_conf: None,
            mounts_toml: None,
            jobs_toml: Some("[[jobs]]\nname = \"nuovo\"\n".to_string()),
            bisync_toml: None,
        })
        .unwrap();

        restore_backup_in(&dir.path, &json).unwrap();

        assert_eq!(std::fs::read_to_string(dir.path.join("jobs.toml")).unwrap(), "[[jobs]]\nname = \"nuovo\"\n");
    }

    #[test]
    fn restore_backup_in_rejects_invalid_json() {
        let dir = TempDir::new("backup-restore-invalid");
        let result = restore_backup_in(&dir.path, "questo non è json valido");
        assert!(result.is_err());
    }

    #[test]
    fn create_then_restore_round_trips_a_realistic_config() {
        let source_dir = TempDir::new("backup-roundtrip-source");
        std::fs::create_dir_all(&source_dir.path).unwrap();
        std::fs::write(source_dir.path.join("rclone.conf"), "[cubbit]\ntype = s3\nprovider = Cubbit\n").unwrap();
        std::fs::write(source_dir.path.join("mounts.toml"), "[[mounts]]\nname = \"cloud\"\nremote = \"cubbit:x\"\nmount_point = \"/home/utente/cloud\"\nauto_mount = false\n").unwrap();

        let json = create_backup_in(&source_dir.path).unwrap();

        let dest_dir = TempDir::new("backup-roundtrip-dest");
        restore_backup_in(&dest_dir.path, &json).unwrap();

        assert_eq!(
            std::fs::read_to_string(dest_dir.path.join("rclone.conf")).unwrap(),
            std::fs::read_to_string(source_dir.path.join("rclone.conf")).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(dest_dir.path.join("mounts.toml")).unwrap(),
            std::fs::read_to_string(source_dir.path.join("mounts.toml")).unwrap()
        );
    }

    #[test]
    fn encrypt_then_decrypt_bundle_round_trips_with_the_right_password() {
        let json = create_backup_in(&TempDir::new("backup-encrypt-roundtrip").path).unwrap();
        let encrypted = encrypt_bundle(&json, "una password niente male").unwrap();
        let decrypted = decrypt_bundle(&encrypted, "una password niente male").unwrap();
        assert_eq!(decrypted, json);
    }

    #[test]
    fn decrypt_bundle_rejects_the_wrong_password() {
        let json = create_backup_in(&TempDir::new("backup-encrypt-wrong-password").path).unwrap();
        let encrypted = encrypt_bundle(&json, "password giusta").unwrap();
        let result = decrypt_bundle(&encrypted, "password sbagliata");
        assert!(result.is_err());
    }

    #[test]
    fn encrypted_bundle_is_not_readable_as_plain_text() {
        let dir = TempDir::new("backup-encrypt-not-plaintext");
        std::fs::create_dir_all(&dir.path).unwrap();
        std::fs::write(dir.path.join("rclone.conf"), "[cubbit]\ntype = s3\nsecret_access_key = molto-segreto\n").unwrap();

        let json = create_backup_in(&dir.path).unwrap();
        let encrypted = encrypt_bundle(&json, "password").unwrap();

        assert!(String::from_utf8(encrypted).is_err(), "il file cifrato non deve essere leggibile come UTF-8/JSON in chiaro");
    }
}
