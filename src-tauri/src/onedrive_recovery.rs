use crate::rcd::{self, RcdState};

/// `true` se il messaggio corrisponde al fallimento noto di Microsoft
/// nell'elencare tutti i drive di un account OneDrive ("Database Is Read
/// Only" / `serviceReadOnly`) — verificato su un caso reale: l'endpoint
/// plurale `/me/drives` (quello che rclone usa per popolare la scelta del
/// drive) può restituire 403 anche quando l'endpoint singolare `/me/drive`
/// (il drive di default dell'utente) risponde normalmente.
pub(crate) fn is_drive_listing_failure(error: &str) -> bool {
    error.contains("Failed to query available drives") || error.contains("serviceReadOnly") || error.contains("Database Is Read Only")
}

fn extract_access_token(remote: &serde_json::Value) -> Result<String, String> {
    let token_json = remote.get("token").and_then(|v| v.as_str()).ok_or("nessun token OAuth salvato per questo remote")?;
    let token: serde_json::Value = serde_json::from_str(token_json).map_err(|e| format!("token OAuth non leggibile: {e}"))?;
    token
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "campo 'access_token' mancante nel token OAuth".to_string())
}

fn extract_drive_id_and_type(drive: &serde_json::Value) -> Result<(String, String), String> {
    let drive_id =
        drive.get("id").and_then(|v| v.as_str()).ok_or("campo 'id' mancante nella risposta di Microsoft Graph")?.to_string();
    let drive_type = drive
        .get("driveType")
        .and_then(|v| v.as_str())
        .ok_or("campo 'driveType' mancante nella risposta di Microsoft Graph")?
        .to_string();
    Ok((drive_id, drive_type))
}

/// Recupera automaticamente da un fallimento di `/me/drives`: legge il
/// token OAuth che rclone ha già salvato nel remote incompleto (`config/get`
/// lo restituisce anche prima che la configurazione sia completa,
/// verificato), interroga direttamente `/me/drive` (singolare, il drive di
/// default — funziona anche quando il plurale fallisce) per ottenere
/// drive_id/drive_type, poi completa il remote con `config/update`.
///
/// Unico punto del progetto in cui si parla direttamente con l'API di un
/// provider invece che tramite rclone: giustificato solo perché qui rclone
/// stessa non offre un modo alternativo di recuperare — l'errore arriva come
/// `Error` secco che interrompe il flusso, non come un'altra domanda a cui
/// rispondere diversamente. Usata solo dal percorso guidato, mai da
/// "Configurazione avanzata": un utente che sceglie quel percorso potrebbe
/// voler scegliere un drive diverso da quello di default (es. un sito
/// SharePoint), non ha senso decidere per lui.
pub(crate) async fn try_recover_drive_id(state: &RcdState, name: &str) -> Result<(), String> {
    let remote = rcd::call(state, "config/get", serde_json::json!({ "name": name })).await?;
    let access_token = extract_access_token(&remote)?;

    let client = reqwest::Client::new();
    let response = client
        .get("https://graph.microsoft.com/v1.0/me/drive")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("impossibile contattare Microsoft Graph: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("Microsoft Graph ha risposto con errore {}", response.status()));
    }

    let drive: serde_json::Value = response.json().await.map_err(|e| format!("risposta di Microsoft Graph non valida: {e}"))?;
    let (drive_id, drive_type) = extract_drive_id_and_type(&drive)?;

    // `opt.nonInteractive` è indispensabile qui: senza, rclone esegue il
    // ciclo di configurazione "da terminale" (legge da stdin, che il nostro
    // processo rcd non ha) invece del protocollo RC a singolo passo —
    // verificato nel sorgente (`fs/config/config.go::updateRemote`), per
    // OneDrive questo rientra sempre nel callback OAuth del backend, che
    // (un token esiste già) chiede "Token already configured - replace
    // it?" e prosegue riaprendo un secondo vero login nel browser, restando
    // bloccato in attesa di risposte che non arriveranno mai. drive_id e
    // drive_type vengono comunque salvati: `updateRemote` li scrive nella
    // mappa di configurazione prima ancora di toccare il callback del
    // backend, indipendentemente da `nonInteractive`.
    rcd::call(
        state,
        "config/update",
        serde_json::json!({
            "name": name,
            "parameters": { "drive_id": drive_id, "drive_type": drive_type },
            "opt": { "nonInteractive": true },
        }),
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_drive_listing_failure_matches_the_known_microsoft_error() {
        assert!(is_drive_listing_failure(
            "Failed to query available drives: HTTP error 403 (403 Forbidden) returned body: \"{\\\"error\\\":{\\\"code\\\":\\\"accessDenied\\\",\\\"message\\\":\\\"Database Is Read Only\\\",\\\"innerError\\\":{\\\"code\\\":\\\"serviceReadOnly\\\"}}}\""
        ));
    }

    #[test]
    fn is_drive_listing_failure_ignores_unrelated_errors() {
        assert!(!is_drive_listing_failure("directory not found"));
        assert!(!is_drive_listing_failure("invalid password"));
    }

    #[test]
    fn extract_access_token_reads_the_nested_json_field() {
        let remote = serde_json::json!({
            "type": "onedrive",
            "token": "{\"access_token\":\"un-token\",\"token_type\":\"Bearer\"}",
        });
        assert_eq!(extract_access_token(&remote).unwrap(), "un-token");
    }

    #[test]
    fn extract_access_token_fails_when_the_token_field_is_missing() {
        let remote = serde_json::json!({"type": "onedrive"});
        assert!(extract_access_token(&remote).is_err());
    }

    #[test]
    fn extract_access_token_fails_on_malformed_json() {
        let remote = serde_json::json!({"type": "onedrive", "token": "non e' json"});
        assert!(extract_access_token(&remote).is_err());
    }

    #[test]
    fn extract_drive_id_and_type_reads_both_fields() {
        let drive = serde_json::json!({"id": "87634c9bd1df88d4", "driveType": "personal"});
        assert_eq!(extract_drive_id_and_type(&drive).unwrap(), ("87634c9bd1df88d4".to_string(), "personal".to_string()));
    }

    #[test]
    fn extract_drive_id_and_type_fails_when_a_field_is_missing() {
        assert!(extract_drive_id_and_type(&serde_json::json!({"driveType": "personal"})).is_err());
        assert!(extract_drive_id_and_type(&serde_json::json!({"id": "x"})).is_err());
    }
}
