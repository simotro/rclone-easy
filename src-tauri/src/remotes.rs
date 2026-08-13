use crate::rcd::{self, RcdState};
use serde::Serialize;
use std::collections::HashMap;
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct S3ProviderOption {
    pub value: String,
    pub help: String,
}

/// Estrae i valori possibili dell'opzione `provider` del backend `s3` dalla
/// risposta di `config/providers` (elenco di tutti i backend di rclone con i
/// rispettivi campi). Presa da qui invece che trascritta a mano: l'elenco
/// (53 provider, incluso "Cubbit") è quello che rclone stesso conosce, resta
/// allineato automaticamente alle versioni future di rclone.
fn extract_s3_providers(body: &serde_json::Value) -> Result<Vec<S3ProviderOption>, String> {
    let providers = body
        .get("providers")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("campo 'providers' mancante nella risposta di rclone rcd: {body}"))?;

    let s3 = providers
        .iter()
        .find(|p| p.get("Name").and_then(|n| n.as_str()) == Some("s3"))
        .ok_or_else(|| "provider 's3' non trovato nella risposta di rclone rcd".to_string())?;

    let provider_option = s3
        .get("Options")
        .and_then(|v| v.as_array())
        .and_then(|opts| opts.iter().find(|o| o.get("Name").and_then(|n| n.as_str()) == Some("provider")))
        .ok_or_else(|| "opzione 'provider' non trovata per il backend s3".to_string())?;

    let examples = provider_option
        .get("Examples")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "campo 'Examples' mancante per l'opzione 'provider' di s3".to_string())?;

    Ok(examples
        .iter()
        .filter_map(|e| {
            let value = e.get("Value")?.as_str()?.to_string();
            let help = e.get("Help").and_then(|h| h.as_str()).unwrap_or("").to_string();
            Some(S3ProviderOption { value, help })
        })
        .collect())
}

#[tauri::command]
pub async fn list_s3_providers(state: tauri::State<'_, RcdState>) -> Result<Vec<S3ProviderOption>, String> {
    let body = rcd::call(&state, "config/providers", serde_json::json!({})).await?;
    extract_s3_providers(&body)
}

#[derive(Debug, Serialize, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct S3RegionOption {
    pub value: String,
    pub help: String,
    /// Nomi dei provider (campo `Provider` di rclone, lista separata da
    /// virgole) per cui questo esempio di regione ha senso — es. una
    /// regione "us-east-1" è specifica di AWS, non va proposta per Cubbit.
    /// Vuoto se rclone non lo specifica (esempio valido per ogni provider).
    pub providers: Vec<String>,
}

/// Stessa idea di `extract_s3_providers`, ma per l'opzione `region` — usata
/// per proporre in UI solo le regioni pertinenti al provider scelto
/// dall'utente, invece di un campo di testo libero senza aiuto.
fn extract_s3_regions(body: &serde_json::Value) -> Result<Vec<S3RegionOption>, String> {
    let providers = body
        .get("providers")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("campo 'providers' mancante nella risposta di rclone rcd: {body}"))?;

    let s3 = providers
        .iter()
        .find(|p| p.get("Name").and_then(|n| n.as_str()) == Some("s3"))
        .ok_or_else(|| "provider 's3' non trovato nella risposta di rclone rcd".to_string())?;

    let region_option = s3
        .get("Options")
        .and_then(|v| v.as_array())
        .and_then(|opts| opts.iter().find(|o| o.get("Name").and_then(|n| n.as_str()) == Some("region")));

    // Non tutte le versioni di rclone garantiscono che l'opzione esista con
    // 'Examples': se manca, restituiamo un elenco vuoto invece di un errore
    // — la UI ricade sul campo di testo libero.
    let Some(region_option) = region_option else { return Ok(Vec::new()) };
    let examples = region_option.get("Examples").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&[]);

    Ok(examples
        .iter()
        .filter_map(|e| {
            let value = e.get("Value")?.as_str()?.to_string();
            let help = e.get("Help").and_then(|h| h.as_str()).unwrap_or("").to_string();
            let providers = e
                .get("Provider")
                .and_then(|p| p.as_str())
                .map(|p| p.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            Some(S3RegionOption { value, help, providers })
        })
        .collect())
}

#[tauri::command]
pub async fn list_s3_regions(state: tauri::State<'_, RcdState>) -> Result<Vec<S3RegionOption>, String> {
    let body = rcd::call(&state, "config/providers", serde_json::json!({})).await?;
    extract_s3_regions(&body)
}

/// Estrae la lista di nomi dalla risposta di `config/listremotes`
/// (`{"remotes": ["nome1", "nome2"]}`) — usata anche da `tray.rs` per
/// elencare i remote senza alcun mount/job/bisync configurato (voce
/// "Configura" nel menu).
pub(crate) fn extract_remote_names(body: &serde_json::Value) -> Result<Vec<String>, String> {
    body.get("remotes")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .ok_or_else(|| format!("campo 'remotes' mancante nella risposta di rclone rcd: {body}"))
}

#[tauri::command]
pub async fn list_own_remotes(state: tauri::State<'_, RcdState>) -> Result<Vec<String>, String> {
    let body = rcd::call(&state, "config/listremotes", serde_json::json!({})).await?;
    extract_remote_names(&body)
}

#[derive(Debug, Serialize, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDirEntry {
    pub name: String,
    /// Percorso completo dalla radice del remote, ricostruito qui
    /// concatenando il `path` di partenza al nome di ogni voce (non è
    /// quello che `operations/list` restituisce da sé in `Path`: dato che
    /// `path` finisce dentro `fs`, non nel parametro `remote` della
    /// chiamata — vedi sotto — `Path` nella risposta è già relativo alla
    /// cartella interrogata, non alla radice del remote). Quello che va
    /// passato come `path` nella chiamata successiva per scendere in questa
    /// cartella, e quello salvato in `source`/`destination`/`path1`/`path2`/
    /// `remote` se l'utente la sceglie.
    pub path: String,
}

/// `fs` porta con sé anche `path` (`nome:percorso`, `remote` della RC call
/// sempre vuoto) invece dello split "canonico" `fs: nome:` + `remote:
/// percorso` — quello split assume un `root` configurato sul backend per
/// risolvere un `remote` assoluto, ma il backend `local` (root implicito
/// "/" del filesystem) non ce l'ha: un percorso assoluto lì dentro produce
/// "directory not found" (verificato a mano contro un demone rclone
/// reale). Concatenare in `fs` funziona per qualunque backend, locale
/// incluso, senza pagare complessità in più per gli altri.
async fn list_remote_dir_in(state: &RcdState, remote: &str, path: &str) -> Result<Vec<RemoteDirEntry>, String> {
    let body = rcd::call(
        state,
        "operations/list",
        serde_json::json!({
            "fs": format!("{remote}:{path}"),
            "remote": "",
            // `recurse: false` esplicito (di suo è già il default, ma su
            // remote grossi/lenti vogliamo essere sicuri che il picker non
            // scateni mai una scansione ricorsiva per sbaglio): il picker
            // nel frontend ora mostra un solo livello alla volta, quindi
            // ogni chiamata deve restare una singola richiesta di elenco,
            // non un attraversamento profondo.
            "opt": { "dirsOnly": true, "noModTime": true, "recurse": false },
        }),
    )
    .await?;

    let list = body.get("list").and_then(|v| v.as_array()).ok_or_else(|| format!("campo 'list' mancante nella risposta di rclone rcd: {body}"))?;

    Ok(list
        .iter()
        .filter_map(|entry| {
            let name = entry.get("Name")?.as_str()?.to_string();
            let full_path = if path.is_empty() { name.clone() } else { format!("{path}/{name}") };
            Some(RemoteDirEntry { name, path: full_path })
        })
        .collect())
}

/// Elenca le sole sottocartelle (non i file) di un remote a un dato
/// percorso — usata dal selettore di cartella remota nel frontend
/// (`RemoteFolderPicker.svelte`) per sfogliare un remote esattamente come
/// il selettore di cartelle locali sfoglia il filesystem. `path` vuoto
/// interroga la radice del remote.
#[tauri::command]
pub async fn list_remote_dir(state: tauri::State<'_, RcdState>, remote: String, path: String) -> Result<Vec<RemoteDirEntry>, String> {
    list_remote_dir_in(&state, &remote, &path).await
}

fn extract_config_path(body: &serde_json::Value) -> Result<String, String> {
    body.get("config")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("campo 'config' mancante nella risposta di rclone rcd: {body}"))
}

/// Percorso su disco della config propria dell'app (mai quella di sistema),
/// mostrato nell'interfaccia così un utente che conosce già rclone sa dove
/// cercarla invece di essere disorientato da un `rclone listremotes` vuoto
/// (che di default legge il rclone.conf di sistema, non quello di Rclone
/// Easy).
#[tauri::command]
pub async fn own_config_path(state: tauri::State<'_, RcdState>) -> Result<String, String> {
    let body = rcd::call(&state, "config/paths", serde_json::json!({})).await?;
    extract_config_path(&body)
}

/// Rifiuta esplicitamente un nome già usato da un remote proprio, invece di
/// lasciare che `config/create` lo sovrascriva silenziosamente (comportamento
/// di rclone stesso, verificato in fase di investigazione — nessun errore,
/// nessun avviso). Riusata sia dall'inserimento manuale/import
/// (`create_remote_in`) sia dal flusso OAuth in `oauth_remote.rs`.
pub(crate) async fn ensure_name_available(state: &RcdState, name: &str) -> Result<(), String> {
    let existing = extract_remote_names(&rcd::call(state, "config/listremotes", serde_json::json!({})).await?)?;
    if existing.contains(&name.to_string()) {
        return Err(format!(
            "esiste già un remote chiamato '{name}' in Rclone Easy: scegli un altro nome per non sovrascriverlo"
        ));
    }
    Ok(())
}

/// Crea un remote nella config propria dell'app e ne verifica subito la
/// connessione con `operations/list`. Se la verifica fallisce, il remote
/// appena creato viene rimosso prima di restituire l'errore: dal punto di
/// vista di chi chiama, un remote non risulta mai salvato se la connessione
/// non funziona (requisito dello SPEC — "test di connessione prima del
/// salvataggio" — senza dover costruire a mano le connection string inline
/// di rclone, altra fonte di bug per parametri con caratteri speciali).
///
/// `obscure` distingue chi chiama: `true` per l'inserimento manuale dal
/// wizard (l'utente digita password in chiaro, rclone deve offuscarle);
/// `false` per l'import da un altro config (`remotes::import_remote`), dove
/// i valori arrivano già offuscati così come letti da `config dump` — se li
/// si offuscasse di nuovo si otterrebbero credenziali sbagliate senza alcun
/// errore visibile (verificato in fase di investigazione).
async fn create_remote_in(
    state: &RcdState,
    name: &str,
    kind: &str,
    parameters: &HashMap<String, String>,
    obscure: bool,
) -> Result<(), String> {
    ensure_name_available(state, name).await?;

    rcd::call(
        state,
        "config/create",
        serde_json::json!({
            "name": name,
            "type": kind,
            "parameters": parameters,
            "opt": { "nonInteractive": true, "obscure": obscure, "noObscure": !obscure },
        }),
    )
    .await?;

    verify_and_cleanup(state, name).await
}

/// Verifica che il remote risponda davvero (`operations/list` sulla root),
/// senza alcuna azione correttiva in caso di fallimento — il chiamante
/// decide cosa fare (cancellare per una creazione, ripristinare i vecchi
/// parametri per una modifica).
async fn verify_connection(state: &RcdState, name: &str) -> Result<(), String> {
    rcd::call(
        state,
        "operations/list",
        serde_json::json!({
            "fs": format!("{name}:"),
            "remote": "",
            "opt": { "recurse": false },
        }),
    )
    .await?;
    Ok(())
}

/// Verifica che il remote appena creato risponda davvero e lo rimuove se la
/// verifica fallisce — dal punto di vista di chi chiama, un remote non
/// risulta mai salvato se la connessione non funziona. Riusata sia da
/// `create_remote_in` (inserimento manuale/import) sia dal flusso OAuth in
/// `oauth_remote.rs`, dopo l'ultimo passo della sua catena di domande.
pub(crate) async fn verify_and_cleanup(state: &RcdState, name: &str) -> Result<(), String> {
    if let Err(connection_error) = verify_connection(state, name).await {
        // Best-effort: se anche la cancellazione fallisce non c'è comunque
        // nulla di più sensato da fare qui che segnalare l'errore originale.
        let _ = rcd::call(state, "config/delete", serde_json::json!({ "name": name })).await;
        return Err(connection_error);
    }

    Ok(())
}

#[tauri::command]
pub async fn create_remote(
    state: tauri::State<'_, RcdState>,
    name: String,
    kind: String,
    parameters: HashMap<String, String>,
) -> Result<(), String> {
    create_remote_in(&state, &name, &kind, &parameters, true).await
}

/// Importa un remote già presente nel `rclone.conf` di sistema nella config
/// propria dell'app, recuperandone tipo e parametri internamente (mai
/// esposti al frontend, vedi `existing_config::dump_remote_parameters`) e
/// riusando lo stesso ciclo crea-verifica-rollback di `create_remote`.
#[tauri::command]
pub async fn import_remote(state: tauri::State<'_, RcdState>, name: String, config_path: Option<String>) -> Result<(), String> {
    let config_path = config_path.as_deref().map(std::path::Path::new);
    let (kind, parameters) = crate::existing_config::dump_remote_parameters(&name, config_path).await?;
    create_remote_in(&state, &name, &kind, &parameters, false).await
}

/// Campi non sensibili per tipo di backend, stesso sottoinsieme già gestito
/// dal wizard (`nuovo-remote`) — gli unici che ha senso mostrare in un form
/// di modifica per la pre-compilazione. I campi password/chiave restano
/// sempre fuori: non attraversano mai il confine verso il frontend (stesso
/// principio di `existing_config::dump_remote_parameters`), l'utente li
/// lascia vuoti nel form se non vuole cambiarli.
fn non_secret_fields_for(kind: &str) -> &'static [&'static str] {
    match kind {
        "s3" => &["provider", "endpoint", "access_key_id", "region"],
        "b2" => &["account"],
        "mega" => &["user"],
        // `client_id` non è un segreto (Google lo tratta come pubblico
        // anche nei client "installed", solo il client_secret è semi-
        // sensibile) — esposto qui così la pagina di modifica può dire
        // all'utente se questo remote Drive usa già un client suo o
        // ancora l'identità condivisa in ritiro nel 2026, senza esporre
        // il client_secret vero e proprio.
        "drive" => &["client_id"],
        "webdav" => &["vendor", "url", "user"],
        "sftp" => &["host", "user", "port"],
        _ => &[],
    }
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteForEdit {
    pub kind: String,
    pub parameters: HashMap<String, String>,
}

async fn get_remote_for_edit_in(state: &RcdState, name: &str) -> Result<RemoteForEdit, String> {
    let dump = rcd::call(state, "config/dump", serde_json::json!({})).await?;
    let (kind, all_parameters) = crate::existing_config::extract_remote_parameters(&dump, name)?;
    let allowed = non_secret_fields_for(&kind);
    let parameters = all_parameters.into_iter().filter(|(k, _)| allowed.contains(&k.as_str())).collect();
    Ok(RemoteForEdit { kind, parameters })
}

/// Tipo e parametri non sensibili di un remote proprio, per pre-compilare il
/// form di modifica. Legge dalla config propria dell'app via RC
/// (`config/dump`), non dal `rclone.conf` di sistema come
/// `existing_config::dump_remote_parameters` (quella serve solo per
/// l'import).
#[tauri::command]
pub async fn get_remote_for_edit(state: tauri::State<'_, RcdState>, name: String) -> Result<RemoteForEdit, String> {
    get_remote_for_edit_in(&state, &name).await
}

/// Applica solo i campi passati in `parameters` (il frontend include solo
/// quelli che l'utente ha effettivamente valorizzato, stesso schema di
/// `buildParameters` nel wizard di creazione) via `config/update`, che a
/// differenza di `config/create` fa un merge parziale sui parametri
/// esistenti (verificato: aggiornare solo `endpoint` lascia `pass`/`user`
/// intatti) — così un campo password lasciato vuoto nel form non cancella
/// né tocca la credenziale già salvata. Se la verifica di connessione
/// fallisce dopo la modifica, i parametri cambiati vengono ripristinati ai
/// valori precedenti (letti prima dell'update, già "obscured" così come
/// salvati — `noObscure:true` per non offuscarli una seconda volta): a
/// differenza della creazione non si può "cancellare" un remote che altri
/// mount/backup/bisync potrebbero già referenziare.
async fn update_remote_in(state: &RcdState, name: &str, parameters: &HashMap<String, String>) -> Result<(), String> {
    let dump = rcd::call(state, "config/dump", serde_json::json!({})).await?;
    let (_, old_parameters) = crate::existing_config::extract_remote_parameters(&dump, name)?;

    rcd::call(
        state,
        "config/update",
        serde_json::json!({
            "name": name,
            "parameters": parameters,
            "opt": { "nonInteractive": true, "obscure": true },
        }),
    )
    .await?;

    if let Err(connection_error) = verify_connection(state, name).await {
        let _ = rcd::call(
            state,
            "config/update",
            serde_json::json!({
                "name": name,
                "parameters": old_parameters,
                "opt": { "nonInteractive": true, "noObscure": true },
            }),
        )
        .await;
        return Err(connection_error);
    }

    Ok(())
}

/// rclone non espone un `config/rename` via RC (investigato in fase di
/// modifica dei remote: non esiste, solo `config/create`/`config/update`/
/// `config/delete` a nome fisso) — rinominare è quindi crea una voce con lo
/// stesso tipo/parametri sotto il nuovo nome (`obscure:false`: i valori
/// arrivano già offuscati dal dump, stesso principio dell'import) +
/// aggiorna ogni mount/backup/bisync che referenzia il vecchio nome perché
/// puntino al nuovo + SOLO alla fine cancella la vecchia voce. L'ordine è
/// deliberato: se un passo qualunque fallisce prima dell'ultimo, il remote
/// vecchio resta comunque presente e funzionante — nessuna voce finisce mai
/// a referenziare un remote inesistente. Chiamata solo da `update_remote`,
/// dopo che `update_remote_in` ha già verificato che il remote (sotto il
/// vecchio nome) risponde con i parametri correnti.
async fn rename_remote_in(state: &RcdState, config_dir: &std::path::Path, old_name: &str, new_name: &str) -> Result<(), String> {
    let dump = rcd::call(state, "config/dump", serde_json::json!({})).await?;
    let (kind, parameters) = crate::existing_config::extract_remote_parameters(&dump, old_name)?;

    create_remote_in(state, new_name, &kind, &parameters, false).await?;

    crate::mounts::rename_remote_references_in(config_dir, old_name, new_name)?;
    crate::jobs::rename_remote_references_in(config_dir, old_name, new_name)?;
    crate::bisync::rename_remote_references_in(config_dir, old_name, new_name)?;

    // Best-effort: a questo punto tutti i riferimenti puntano già al nuovo
    // nome, quindi un fallimento qui lascia solo una voce duplicata e
    // inutilizzata (cosmetica, non rompe nulla) — stesso principio già
    // seguito in `verify_and_cleanup`.
    let _ = rcd::call(state, "config/delete", serde_json::json!({ "name": old_name })).await;
    Ok(())
}

/// `old_name`/`name` seguono la stessa convenzione già usata da
/// mount/backup/bisync (`update_mount`/`update_job`/`update_bisync_job`):
/// `old_name` individua il remote da modificare, `name` è il nome dopo il
/// salvataggio, diverso da `old_name` solo se l'utente lo ha rinominato.
/// L'aggiornamento dei parametri avviene SEMPRE sotto `old_name` per primo
/// — nessun rischio di rinominare prima di sapere se la connessione
/// sopravvive ai nuovi parametri — e solo se quello riesce, e solo se il
/// nome è davvero cambiato, si procede con `rename_remote_in`.
#[tauri::command]
pub async fn update_remote(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    old_name: String,
    name: String,
    parameters: HashMap<String, String>,
) -> Result<(), String> {
    update_remote_in(&state, &old_name, &parameters).await?;

    if name != old_name {
        let config_dir =
            app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))?;
        rename_remote_in(&state, &config_dir, &old_name, &name).await?;
    }

    Ok(())
}

fn references_remote(fs: &str, remote_name: &str) -> bool {
    fs.starts_with(&format!("{remote_name}:"))
}

/// Rifiuta l'eliminazione se il remote è ancora referenziato da un mount,
/// un backup o una sincronizzazione bidirezionale — eliminarlo lascerebbe
/// quelle voci puntare a un remote inesistente, con errori poco chiari alla
/// prima esecuzione invece che un rifiuto immediato e comprensibile. Il
/// messaggio nomina la voce che blocca (es. "il mount 'mnt-wasabi'"): senza
/// il nome l'utente non ha modo di sapere cosa cercare per sbloccare
/// l'eliminazione, specialmente se quella voce non è ovvia dall'interfaccia.
fn check_remote_not_in_use(config_dir: &std::path::Path, name: &str) -> Result<(), String> {
    if let Some(mount) = crate::mounts::load_from_dir(config_dir).unwrap_or_default().into_iter().find(|m| references_remote(&m.remote, name)) {
        return Err(format!("'{name}' è usato dal mount '{}': eliminalo prima di rimuovere il remote", mount.name));
    }
    if let Some(job) = crate::jobs::load_from_dir(config_dir)
        .unwrap_or_default()
        .into_iter()
        .find(|j| references_remote(&j.source, name) || references_remote(&j.destination, name))
    {
        return Err(format!("'{name}' è usato dal backup '{}': eliminalo prima di rimuovere il remote", job.name));
    }
    if let Some(job) = crate::bisync::load_from_dir(config_dir)
        .unwrap_or_default()
        .into_iter()
        .find(|j| references_remote(&j.path1, name) || references_remote(&j.path2, name))
    {
        return Err(format!(
            "'{name}' è usato dalla sincronizzazione bidirezionale '{}': eliminala prima di rimuovere il remote",
            job.name
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_remote(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<(), String> {
    let config_dir =
        app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))?;
    check_remote_not_in_use(&config_dir, &name)?;
    rcd::call(&state, "config/delete", serde_json::json!({ "name": name })).await?;
    Ok(())
}

/// Nomi delle voci (mount/backup/bisync) che referenziano un remote, per
/// mostrarle nel modal di conferma prima di un'eliminazione a cascata —
/// `None` per un tipo di servizio non configurato per questo remote.
#[derive(Debug, Serialize, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RemoteUsage {
    pub mount_name: Option<String>,
    pub backup_name: Option<String>,
    pub bisync_name: Option<String>,
}

fn remote_usage_in(config_dir: &std::path::Path, name: &str) -> RemoteUsage {
    let mount_name = crate::mounts::load_from_dir(config_dir)
        .unwrap_or_default()
        .into_iter()
        .find(|m| references_remote(&m.remote, name))
        .map(|m| m.name);
    let backup_name = crate::jobs::load_from_dir(config_dir)
        .unwrap_or_default()
        .into_iter()
        .find(|j| references_remote(&j.source, name) || references_remote(&j.destination, name))
        .map(|j| j.name);
    let bisync_name = crate::bisync::load_from_dir(config_dir)
        .unwrap_or_default()
        .into_iter()
        .find(|j| references_remote(&j.path1, name) || references_remote(&j.path2, name))
        .map(|j| j.name);
    RemoteUsage { mount_name, backup_name, bisync_name }
}

#[tauri::command]
pub async fn remote_usage(app: AppHandle, name: String) -> Result<RemoteUsage, String> {
    let config_dir =
        app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))?;
    Ok(remote_usage_in(&config_dir, &name))
}

/// Elimina il remote insieme a mount/backup/bisync che lo referenziano
/// (al più uno per tipo, per via del vincolo "un solo mount/backup/bisync
/// per remote" già applicato altrove) — usata dal modal di conferma in cui
/// l'utente ha già visto ed accettato cosa verrà rimosso insieme al
/// remote, quindi qui non c'è più bisogno del rifiuto secco di
/// `check_remote_not_in_use`/`delete_remote`. Un mount ancora montato
/// viene smontato prima di rimuoverne la configurazione, per non lasciare
/// un mount FUSE "stale" (stesso principio di `rcd::shutdown`).
async fn delete_remote_cascade_in(state: &RcdState, config_dir: &std::path::Path, name: &str) -> Result<(), String> {
    if let Some(mount) =
        crate::mounts::load_from_dir(config_dir).unwrap_or_default().into_iter().find(|m| references_remote(&m.remote, name))
    {
        let active = crate::mounts::active_mount_points(state).await.unwrap_or_default();
        if crate::mounts::is_mounted(&active, &mount.mount_point) {
            rcd::call(state, "mount/unmount", serde_json::json!({ "mountPoint": mount.mount_point })).await?;
        }
        crate::mounts::delete_mount_in(config_dir, &mount.name)?;
    }

    if let Some(job) = crate::jobs::load_from_dir(config_dir)
        .unwrap_or_default()
        .into_iter()
        .find(|j| references_remote(&j.source, name) || references_remote(&j.destination, name))
    {
        crate::jobs::delete_job_in(config_dir, &job.name)?;
    }

    if let Some(job) = crate::bisync::load_from_dir(config_dir)
        .unwrap_or_default()
        .into_iter()
        .find(|j| references_remote(&j.path1, name) || references_remote(&j.path2, name))
    {
        crate::bisync::delete_bisync_job_in(config_dir, &job.name)?;
    }

    rcd::call(state, "config/delete", serde_json::json!({ "name": name })).await?;
    Ok(())
}

#[tauri::command]
pub async fn delete_remote_cascade(app: AppHandle, state: tauri::State<'_, RcdState>, name: String) -> Result<(), String> {
    let config_dir =
        app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))?;
    delete_remote_cascade_in(&state, &config_dir, &name).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;
    use std::sync::Mutex as StdMutex;

    // config/create e operations/list toccano lo stesso file rclone.conf:
    // eseguire questi test in parallelo su istanze diverse va bene (ognuna
    // ha il proprio TempDir), ma un lock evita comunque sorprese se in
    // futuro si aggiungono test che condividono uno stesso demone.
    static SUITE_LOCK: StdMutex<()> = StdMutex::new(());

    #[tokio::test]
    async fn create_remote_of_type_local_succeeds_and_is_listed() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-local-ok");
        let state = rcd::build_state(dir.config_path()).await;

        let result = create_remote_in(&state, "rclone-easy-test", "local", &HashMap::new(), true).await;
        assert!(result.is_ok(), "creazione remote locale dovrebbe riuscire: {result:?}");

        let body = rcd::call(&state, "config/listremotes", serde_json::json!({})).await.unwrap();
        let names = extract_remote_names(&body).unwrap();
        assert!(names.contains(&"rclone-easy-test".to_string()));
    }

    #[tokio::test]
    async fn create_remote_rolls_back_when_connection_check_fails() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-http-fail");
        let state = rcd::build_state(dir.config_path()).await;

        // Backend "http" puntato su una porta locale su cui non ascolta
        // nessuno: la connessione viene rifiutata subito, senza rete reale
        // e senza bisogno di credenziali — fallimento deterministico e
        // veloce per operations/list durante la verifica.
        let mut parameters = HashMap::new();
        parameters.insert("url".to_string(), "http://127.0.0.1:1/".to_string());
        let name = "rclone-easy-test-fail";

        let result = create_remote_in(&state, name, "http", &parameters, true).await;
        assert!(result.is_err(), "la creazione dovrebbe fallire per connessione rifiutata: {result:?}");

        let body = rcd::call(&state, "config/listremotes", serde_json::json!({})).await.unwrap();
        let names = extract_remote_names(&body).unwrap();
        assert!(!names.contains(&name.to_string()), "il remote non deve restare salvato dopo un fallimento");
    }

    /// I nuovi backend (WebDAV/Nextcloud/ownCloud/SFTP) passano dallo stesso
    /// `create_remote_in` generico degli altri — verifica che il ciclo
    /// crea-verifica-rollback funzioni per davvero anche per loro, non solo
    /// che compilino: stesso schema del test "http" sopra, connessione
    /// rifiutata deterministica e veloce.
    #[tokio::test]
    async fn create_remote_of_type_webdav_rolls_back_when_connection_check_fails() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-webdav-fail");
        let state = rcd::build_state(dir.config_path()).await;

        let mut parameters = HashMap::new();
        parameters.insert("url".to_string(), "http://127.0.0.1:1/".to_string());
        parameters.insert("vendor".to_string(), "nextcloud".to_string());
        let name = "rclone-easy-test-webdav-fail";

        let result = create_remote_in(&state, name, "webdav", &parameters, true).await;
        assert!(result.is_err(), "la creazione dovrebbe fallire per connessione rifiutata: {result:?}");

        let body = rcd::call(&state, "config/listremotes", serde_json::json!({})).await.unwrap();
        assert!(!extract_remote_names(&body).unwrap().contains(&name.to_string()));
    }

    #[tokio::test]
    async fn create_remote_of_type_sftp_rolls_back_when_connection_check_fails() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-sftp-fail");
        let state = rcd::build_state(dir.config_path()).await;

        let mut parameters = HashMap::new();
        parameters.insert("host".to_string(), "127.0.0.1".to_string());
        parameters.insert("port".to_string(), "1".to_string());
        parameters.insert("user".to_string(), "prova".to_string());
        let name = "rclone-easy-test-sftp-fail";

        let result = create_remote_in(&state, name, "sftp", &parameters, true).await;
        assert!(result.is_err(), "la creazione dovrebbe fallire per connessione rifiutata: {result:?}");

        let body = rcd::call(&state, "config/listremotes", serde_json::json!({})).await.unwrap();
        assert!(!extract_remote_names(&body).unwrap().contains(&name.to_string()));
    }

    #[tokio::test]
    async fn create_remote_refuses_to_overwrite_an_existing_name() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-no-overwrite");
        let state = rcd::build_state(dir.config_path()).await;

        create_remote_in(&state, "rclone-easy-dup", "local", &HashMap::new(), true)
            .await
            .expect("prima creazione dovrebbe riuscire");

        let result = create_remote_in(&state, "rclone-easy-dup", "local", &HashMap::new(), true).await;
        assert!(result.is_err(), "una seconda creazione con lo stesso nome deve essere rifiutata");
    }

    /// Verifica l'opt `obscure`/`noObscure` di `config/create` in isolamento
    /// dal ciclo verifica-connessione/rollback di `create_remote_in` (con
    /// credenziali finte "mega" quel passo fallirebbe sempre e cancellerebbe
    /// il remote prima di poterne leggere il valore salvato) — chiama
    /// `rcd::call` direttamente, stesso schema già validato manualmente
    /// via curl in fase di investigazione.
    #[tokio::test]
    async fn config_create_with_no_obscure_round_trips_an_already_obscured_secret() {
        let _guard = SUITE_LOCK.lock().unwrap();

        let source_dir = TempDir::new("remotes-obscure-source");
        let source_state = rcd::build_state(source_dir.config_path()).await;
        rcd::call(
            &source_state,
            "config/create",
            serde_json::json!({
                "name": "sorgente",
                "type": "mega",
                "parameters": { "user": "a@b.com", "pass": "password-finta-di-test" },
                "opt": { "nonInteractive": true, "obscure": true },
            }),
        )
        .await
        .unwrap();
        let source_dump = rcd::call(&source_state, "config/dump", serde_json::json!({})).await.unwrap();
        let obscured_pass =
            source_dump["sorgente"]["pass"].as_str().expect("il dump dovrebbe contenere 'pass'").to_string();
        assert_ne!(obscured_pass, "password-finta-di-test", "il valore salvato deve essere offuscato, non in chiaro");

        let dest_dir = TempDir::new("remotes-obscure-dest");
        let dest_state = rcd::build_state(dest_dir.config_path()).await;
        rcd::call(
            &dest_state,
            "config/create",
            serde_json::json!({
                "name": "destinazione",
                "type": "mega",
                "parameters": { "user": "a@b.com", "pass": obscured_pass },
                "opt": { "nonInteractive": true, "noObscure": true },
            }),
        )
        .await
        .unwrap();
        let dest_dump = rcd::call(&dest_state, "config/dump", serde_json::json!({})).await.unwrap();
        let dest_pass = dest_dump["destinazione"]["pass"].as_str().unwrap();

        assert_eq!(dest_pass, obscured_pass, "con noObscure:true il valore deve restare identico, non ri-offuscato");
    }

    #[test]
    fn extract_remote_names_reads_the_remotes_field() {
        let body = serde_json::json!({"remotes": ["a", "b"]});
        assert_eq!(extract_remote_names(&body).unwrap(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn extract_remote_names_fails_when_field_missing() {
        let body = serde_json::json!({"other": []});
        assert!(extract_remote_names(&body).is_err());
    }

    fn sample_providers_body() -> serde_json::Value {
        serde_json::json!({
            "providers": [
                {
                    "Name": "mega",
                    "Options": [
                        { "Name": "user", "Required": true }
                    ]
                },
                {
                    "Name": "s3",
                    "Options": [
                        {
                            "Name": "provider",
                            "Examples": [
                                { "Value": "AWS", "Help": "Amazon Web Services (AWS) S3" },
                                { "Value": "Cubbit", "Help": "Cubbit DS3" },
                                { "Value": "Other", "Help": "Any other S3 compatible provider" }
                            ]
                        },
                        { "Name": "access_key_id", "Required": false },
                        {
                            "Name": "region",
                            "Examples": [
                                { "Value": "us-east-1", "Help": "US Region", "Provider": "AWS" },
                                { "Value": "eu-west-1", "Help": "EU (Ireland)", "Provider": "AWS" },
                                { "Value": "garage", "Help": "Any Garage region", "Provider": "Other" }
                            ]
                        }
                    ]
                }
            ]
        })
    }

    #[test]
    fn extract_s3_providers_reads_the_provider_examples() {
        let providers = extract_s3_providers(&sample_providers_body()).unwrap();
        assert_eq!(
            providers,
            vec![
                S3ProviderOption { value: "AWS".to_string(), help: "Amazon Web Services (AWS) S3".to_string() },
                S3ProviderOption { value: "Cubbit".to_string(), help: "Cubbit DS3".to_string() },
                S3ProviderOption {
                    value: "Other".to_string(),
                    help: "Any other S3 compatible provider".to_string()
                },
            ]
        );
    }

    #[test]
    fn extract_s3_providers_fails_when_s3_backend_missing() {
        let body = serde_json::json!({"providers": [{"Name": "mega", "Options": []}]});
        assert!(extract_s3_providers(&body).is_err());
    }

    #[test]
    fn extract_s3_regions_reads_the_region_examples_with_their_provider_filter() {
        let regions = extract_s3_regions(&sample_providers_body()).unwrap();
        assert_eq!(
            regions,
            vec![
                S3RegionOption {
                    value: "us-east-1".to_string(),
                    help: "US Region".to_string(),
                    providers: vec!["AWS".to_string()],
                },
                S3RegionOption {
                    value: "eu-west-1".to_string(),
                    help: "EU (Ireland)".to_string(),
                    providers: vec!["AWS".to_string()],
                },
                S3RegionOption {
                    value: "garage".to_string(),
                    help: "Any Garage region".to_string(),
                    providers: vec!["Other".to_string()],
                },
            ]
        );
    }

    #[test]
    fn extract_s3_regions_is_empty_when_the_option_is_missing_instead_of_failing() {
        let body = serde_json::json!({"providers": [{"Name": "s3", "Options": [{"Name": "provider", "Examples": []}]}]});
        assert_eq!(extract_s3_regions(&body).unwrap(), Vec::new());
    }

    #[tokio::test]
    async fn list_s3_providers_returns_cubbit_from_the_real_daemon() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("s3-providers");
        let state = rcd::build_state(dir.config_path()).await;

        let body = rcd::call(&state, "config/providers", serde_json::json!({})).await.unwrap();
        let providers = extract_s3_providers(&body).unwrap();

        assert!(providers.iter().any(|p| p.value == "Cubbit"));
        assert!(providers.iter().any(|p| p.value == "Other"));
    }

    #[test]
    fn extract_config_path_reads_the_config_field() {
        let body = serde_json::json!({"config": "/home/utente/.config/RcloneEasy/rclone.conf", "cache": "x", "temp": "y"});
        assert_eq!(extract_config_path(&body).unwrap(), "/home/utente/.config/RcloneEasy/rclone.conf");
    }

    #[test]
    fn extract_config_path_fails_when_field_missing() {
        let body = serde_json::json!({"cache": "x"});
        assert!(extract_config_path(&body).is_err());
    }

    #[tokio::test]
    async fn own_config_path_reports_the_temp_dir_config_used_by_the_daemon() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("config-path");
        let expected = dir.config_path();
        let state = rcd::build_state(expected.clone()).await;

        let body = rcd::call(&state, "config/paths", serde_json::json!({})).await.unwrap();
        let reported = extract_config_path(&body).unwrap();

        assert_eq!(std::path::Path::new(&reported), expected);
    }

    #[tokio::test]
    async fn config_update_merges_only_given_parameters_leaving_others_untouched() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-update-merge");
        let state = rcd::build_state(dir.config_path()).await;

        rcd::call(
            &state,
            "config/create",
            serde_json::json!({
                "name": "prova",
                "type": "webdav",
                "parameters": {"url": "https://vecchio.test", "user": "utente", "pass": "segreto", "vendor": "other"},
                "opt": {"nonInteractive": true, "obscure": true},
            }),
        )
        .await
        .unwrap();

        let mut only_url = HashMap::new();
        only_url.insert("url".to_string(), "https://nuovo.test".to_string());
        rcd::call(
            &state,
            "config/update",
            serde_json::json!({ "name": "prova", "parameters": only_url, "opt": { "nonInteractive": true, "obscure": true } }),
        )
        .await
        .unwrap();

        let dump = rcd::call(&state, "config/dump", serde_json::json!({})).await.unwrap();
        assert_eq!(dump["prova"]["url"].as_str().unwrap(), "https://nuovo.test");
        assert_eq!(dump["prova"]["user"].as_str().unwrap(), "utente", "campi non toccati dall'update non devono sparire");
        assert!(!dump["prova"]["pass"].as_str().unwrap().is_empty(), "la password non toccata deve restare presente");
    }

    #[tokio::test]
    async fn update_remote_succeeds_and_keeps_the_remote_when_verification_passes() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-update-ok");
        let state = rcd::build_state(dir.config_path()).await;

        create_remote_in(&state, "prova", "local", &HashMap::new(), true).await.unwrap();

        let result = update_remote_in(&state, "prova", &HashMap::new()).await;
        assert!(result.is_ok(), "un update che non rompe la connessione dovrebbe riuscire: {result:?}");

        let body = rcd::call(&state, "config/listremotes", serde_json::json!({})).await.unwrap();
        let names = extract_remote_names(&body).unwrap();
        assert!(names.contains(&"prova".to_string()), "il remote deve restare presente dopo un update riuscito");
    }

    #[tokio::test]
    async fn update_remote_rolls_back_to_old_parameters_when_connection_check_fails() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-update-rollback");
        let state = rcd::build_state(dir.config_path()).await;

        // Creato direttamente via RC, bypassando create_remote_in: qui non
        // interessa che la connessione iniziale riesca davvero, solo che i
        // parametri "vecchi" siano distinguibili da quelli "nuovi" per
        // verificare il ripristino dopo un update fallito.
        rcd::call(
            &state,
            "config/create",
            serde_json::json!({
                "name": "prova",
                "type": "webdav",
                "parameters": {"url": "http://127.0.0.1:1/vecchio", "vendor": "other"},
                "opt": {"nonInteractive": true, "obscure": true},
            }),
        )
        .await
        .unwrap();

        let mut new_params = HashMap::new();
        new_params.insert("url".to_string(), "http://127.0.0.1:1/nuovo".to_string());
        let result = update_remote_in(&state, "prova", &new_params).await;
        assert!(result.is_err(), "l'update dovrebbe fallire: nessuna connessione reale disponibile su quella porta");

        let dump = rcd::call(&state, "config/dump", serde_json::json!({})).await.unwrap();
        assert_eq!(
            dump["prova"]["url"].as_str().unwrap(),
            "http://127.0.0.1:1/vecchio",
            "dopo un fallimento i parametri devono tornare quelli precedenti"
        );
    }

    #[tokio::test]
    async fn get_remote_for_edit_exposes_only_non_secret_fields() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-get-for-edit");
        let state = rcd::build_state(dir.config_path()).await;

        rcd::call(
            &state,
            "config/create",
            serde_json::json!({
                "name": "prova",
                "type": "s3",
                "parameters": {
                    "provider": "Other", "endpoint": "s3.esempio.test", "region": "eu-west-1",
                    "access_key_id": "chiave-pubblica", "secret_access_key": "segreto-privato"
                },
                "opt": {"nonInteractive": true, "obscure": true},
            }),
        )
        .await
        .unwrap();

        let result = get_remote_for_edit_in(&state, "prova").await.unwrap();
        assert_eq!(result.kind, "s3");
        assert_eq!(result.parameters.get("endpoint"), Some(&"s3.esempio.test".to_string()));
        assert_eq!(result.parameters.get("region"), Some(&"eu-west-1".to_string()));
        assert_eq!(result.parameters.get("access_key_id"), Some(&"chiave-pubblica".to_string()));
        assert!(!result.parameters.contains_key("secret_access_key"), "il segreto non deve mai arrivare al frontend");
    }

    #[tokio::test]
    async fn get_remote_for_edit_exposes_the_drive_client_id_but_not_its_secret() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-get-for-edit-drive");
        let state = rcd::build_state(dir.config_path()).await;

        rcd::call(
            &state,
            "config/create",
            serde_json::json!({
                "name": "drive-prova",
                "type": "drive",
                "parameters": { "client_id": "un-client-id.apps.googleusercontent.com", "client_secret": "segreto-privato" },
                "opt": {"nonInteractive": true, "obscure": true},
            }),
        )
        .await
        .unwrap();

        let result = get_remote_for_edit_in(&state, "drive-prova").await.unwrap();
        assert_eq!(result.kind, "drive");
        assert_eq!(result.parameters.get("client_id"), Some(&"un-client-id.apps.googleusercontent.com".to_string()));
        assert!(!result.parameters.contains_key("client_secret"), "il client_secret non deve mai arrivare al frontend");
    }

    #[tokio::test]
    async fn get_remote_for_edit_exposes_webdav_fields_but_not_the_password() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-get-for-edit-webdav");
        let state = rcd::build_state(dir.config_path()).await;

        rcd::call(
            &state,
            "config/create",
            serde_json::json!({
                "name": "nextcloud-prova",
                "type": "webdav",
                "parameters": {
                    "vendor": "nextcloud", "url": "https://cloud.esempio.test/remote.php/dav/files/utente/",
                    "user": "utente", "pass": "segreto-privato"
                },
                "opt": {"nonInteractive": true, "obscure": true},
            }),
        )
        .await
        .unwrap();

        let result = get_remote_for_edit_in(&state, "nextcloud-prova").await.unwrap();
        assert_eq!(result.kind, "webdav");
        assert_eq!(result.parameters.get("vendor"), Some(&"nextcloud".to_string()));
        assert_eq!(result.parameters.get("url"), Some(&"https://cloud.esempio.test/remote.php/dav/files/utente/".to_string()));
        assert_eq!(result.parameters.get("user"), Some(&"utente".to_string()));
        assert!(!result.parameters.contains_key("pass"), "la password non deve mai arrivare al frontend");
    }

    #[tokio::test]
    async fn get_remote_for_edit_exposes_sftp_fields_but_not_the_password() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remotes-get-for-edit-sftp");
        let state = rcd::build_state(dir.config_path()).await;

        rcd::call(
            &state,
            "config/create",
            serde_json::json!({
                "name": "sftp-prova",
                "type": "sftp",
                "parameters": { "host": "esempio.test", "user": "utente", "port": "2222", "pass": "segreto-privato" },
                "opt": {"nonInteractive": true, "obscure": true},
            }),
        )
        .await
        .unwrap();

        let result = get_remote_for_edit_in(&state, "sftp-prova").await.unwrap();
        assert_eq!(result.kind, "sftp");
        assert_eq!(result.parameters.get("host"), Some(&"esempio.test".to_string()));
        assert_eq!(result.parameters.get("port"), Some(&"2222".to_string()));
        assert!(!result.parameters.contains_key("pass"), "la password non deve mai arrivare al frontend");
    }

    #[test]
    fn check_remote_not_in_use_allows_deletion_when_nothing_references_it() {
        let dir = TempDir::new("remotes-in-use-ok");
        assert!(check_remote_not_in_use(&dir.path, "qualunque-nome").is_ok());
    }

    #[test]
    fn check_remote_not_in_use_blocks_deletion_when_a_mount_references_it() {
        let dir = TempDir::new("remotes-in-use-mount");
        crate::mounts::create_mount_in(&dir.path, "un-mount", "prova:cartella", "/tmp/x", false).unwrap();
        let result = check_remote_not_in_use(&dir.path, "prova");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("mount"));
    }

    #[test]
    fn check_remote_not_in_use_blocks_deletion_when_a_backup_references_it() {
        let dir = TempDir::new("remotes-in-use-job");
        crate::jobs::create_job_in(&dir.path, "un-job", "/tmp/x", "prova:cartella", None, false).unwrap();
        let result = check_remote_not_in_use(&dir.path, "prova");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("backup"));
    }

    #[test]
    fn check_remote_not_in_use_blocks_deletion_when_a_bisync_job_references_it() {
        let dir = TempDir::new("remotes-in-use-bisync");
        crate::bisync::create_bisync_job_in(&dir.path, "un-bisync", "/tmp/x", "prova:cartella", None).unwrap();
        let result = check_remote_not_in_use(&dir.path, "prova");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("bidirezionale"));
    }

    #[test]
    fn remote_usage_in_reports_nothing_when_the_remote_is_free() {
        let dir = TempDir::new("remote-usage-free");
        let usage = remote_usage_in(&dir.path, "prova");
        assert_eq!(usage, RemoteUsage { mount_name: None, backup_name: None, bisync_name: None });
    }

    #[test]
    fn remote_usage_in_names_every_kind_of_reference() {
        let dir = TempDir::new("remote-usage-full");
        crate::mounts::create_mount_in(&dir.path, "un-mount", "prova:x", "/tmp/a", false).unwrap();
        crate::jobs::create_job_in(&dir.path, "un-backup", "/tmp/b", "prova:y", None, false).unwrap();
        crate::bisync::create_bisync_job_in(&dir.path, "un-bisync", "/tmp/c", "prova:z", None).unwrap();

        let usage = remote_usage_in(&dir.path, "prova");
        assert_eq!(
            usage,
            RemoteUsage {
                mount_name: Some("un-mount".to_string()),
                backup_name: Some("un-backup".to_string()),
                bisync_name: Some("un-bisync".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn delete_remote_cascade_in_removes_the_remote_and_everything_referencing_it() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remote-cascade-delete");
        let state = rcd::build_state(dir.config_path()).await;
        create_remote_in(&state, "prova", "local", &HashMap::new(), true).await.unwrap();
        crate::mounts::create_mount_in(&dir.path, "un-mount", "prova:", "/tmp/cascade-mount", false).unwrap();
        crate::jobs::create_job_in(&dir.path, "un-backup", "/tmp/cascade-src", "prova:dest", None, false).unwrap();
        crate::bisync::create_bisync_job_in(&dir.path, "un-bisync", "/tmp/cascade-bisync", "prova:altro", None).unwrap();

        delete_remote_cascade_in(&state, &dir.path, "prova").await.unwrap();

        let body = rcd::call(&state, "config/listremotes", serde_json::json!({})).await.unwrap();
        assert!(!extract_remote_names(&body).unwrap().contains(&"prova".to_string()));
        assert!(crate::mounts::load_from_dir(&dir.path).unwrap().is_empty());
        assert!(crate::jobs::load_from_dir(&dir.path).unwrap().is_empty());
        assert!(crate::bisync::load_from_dir(&dir.path).unwrap().is_empty());
    }

    #[tokio::test]
    async fn rename_remote_in_moves_the_remote_and_updates_every_reference() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remote-rename-full");
        let state = rcd::build_state(dir.config_path()).await;
        create_remote_in(&state, "vecchio", "local", &HashMap::new(), true).await.unwrap();
        crate::mounts::create_mount_in(&dir.path, "un-mount", "vecchio:cartella", "/tmp/rename-mount", false).unwrap();
        crate::jobs::create_job_in(&dir.path, "un-backup", "/tmp/rename-src", "vecchio:dest", None, false).unwrap();
        crate::bisync::create_bisync_job_in(&dir.path, "un-bisync", "/tmp/rename-bisync", "vecchio:altro", None).unwrap();

        rename_remote_in(&state, &dir.path, "vecchio", "nuovo").await.unwrap();

        let body = rcd::call(&state, "config/listremotes", serde_json::json!({})).await.unwrap();
        let names = extract_remote_names(&body).unwrap();
        assert!(names.contains(&"nuovo".to_string()), "il nuovo nome deve esistere: {names:?}");
        assert!(!names.contains(&"vecchio".to_string()), "il vecchio nome non deve più esistere: {names:?}");

        assert_eq!(crate::mounts::load_from_dir(&dir.path).unwrap()[0].remote, "nuovo:cartella");
        assert_eq!(crate::jobs::load_from_dir(&dir.path).unwrap()[0].destination, "nuovo:dest");
        assert_eq!(crate::bisync::load_from_dir(&dir.path).unwrap()[0].path2, "nuovo:altro");
    }

    #[tokio::test]
    async fn rename_remote_in_fails_and_changes_nothing_when_the_new_name_is_already_taken() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remote-rename-conflict");
        let state = rcd::build_state(dir.config_path()).await;
        create_remote_in(&state, "a", "local", &HashMap::new(), true).await.unwrap();
        create_remote_in(&state, "b", "local", &HashMap::new(), true).await.unwrap();
        crate::mounts::create_mount_in(&dir.path, "un-mount", "a:cartella", "/tmp/rename-conflict-mount", false).unwrap();

        let result = rename_remote_in(&state, &dir.path, "a", "b").await;
        assert!(result.is_err(), "rinominare su un nome già usato deve fallire: {result:?}");

        let body = rcd::call(&state, "config/listremotes", serde_json::json!({})).await.unwrap();
        let names = extract_remote_names(&body).unwrap();
        assert!(names.contains(&"a".to_string()) && names.contains(&"b".to_string()), "entrambi i remote devono restare intatti: {names:?}");
        assert_eq!(
            crate::mounts::load_from_dir(&dir.path).unwrap()[0].remote,
            "a:cartella",
            "un rename fallito non deve toccare i riferimenti esistenti"
        );
    }

    #[tokio::test]
    async fn rename_remote_in_succeeds_for_a_remote_with_nothing_referencing_it() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remote-rename-free");
        let state = rcd::build_state(dir.config_path()).await;
        create_remote_in(&state, "solo", "local", &HashMap::new(), true).await.unwrap();

        let result = rename_remote_in(&state, &dir.path, "solo", "solo-nuovo").await;
        assert!(result.is_ok(), "rinominare un remote senza riferimenti dovrebbe riuscire: {result:?}");
    }

    #[tokio::test]
    async fn delete_remote_cascade_in_succeeds_for_a_remote_with_nothing_referencing_it() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remote-cascade-delete-free");
        let state = rcd::build_state(dir.config_path()).await;
        create_remote_in(&state, "prova-libero", "local", &HashMap::new(), true).await.unwrap();

        let result = delete_remote_cascade_in(&state, &dir.path, "prova-libero").await;
        assert!(result.is_ok(), "eliminare un remote senza nulla che lo referenzia dovrebbe riuscire: {result:?}");
    }

    #[tokio::test]
    async fn list_remote_dir_lists_only_subdirectories_of_a_real_local_backend() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remote-dir-list");
        let state = rcd::build_state(dir.config_path()).await;
        create_remote_in(&state, "rclone-easy-test-dir", "local", &HashMap::new(), true).await.unwrap();

        let root = TempDir::new("remote-dir-list-root");
        std::fs::create_dir_all(root.path.join("Backups")).unwrap();
        std::fs::create_dir_all(root.path.join("Photos")).unwrap();
        std::fs::write(root.path.join("nota.txt"), "non è una cartella").unwrap();

        let entries = list_remote_dir_in(&state, "rclone-easy-test-dir", &root.path.to_string_lossy()).await.unwrap();

        let mut names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["Backups".to_string(), "Photos".to_string()], "solo le sottocartelle, non il file");
    }

    #[tokio::test]
    async fn list_remote_dir_in_joins_the_full_path_when_descending_into_a_subfolder() {
        let _guard = SUITE_LOCK.lock().unwrap();
        let dir = TempDir::new("remote-dir-list-nested");
        let state = rcd::build_state(dir.config_path()).await;
        create_remote_in(&state, "rclone-easy-test-nested", "local", &HashMap::new(), true).await.unwrap();

        let root = TempDir::new("remote-dir-list-nested-root");
        std::fs::create_dir_all(root.path.join("Backups/2024")).unwrap();

        // Prima chiamata come farebbe il picker aprendo la radice: `path`
        // vuoto, `entries[0].path` deve essere già pronto da ripassare come
        // `path` per scendere dentro "Backups".
        let top = list_remote_dir_in(&state, "rclone-easy-test-nested", &root.path.to_string_lossy()).await.unwrap();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].name, "Backups");

        // `top[0].path` è già il percorso completo (root.path + "Backups"):
        // esattamente quello che il picker ripasserebbe come `path` per
        // scendere, senza concatenare altro.
        let nested_path = top[0].path.clone();
        let nested = list_remote_dir_in(&state, "rclone-easy-test-nested", &nested_path).await.unwrap();
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0].name, "2024");
        assert_eq!(nested[0].path, format!("{nested_path}/2024"), "il percorso restituito deve essere quello completo, non solo il nome");
    }
}
