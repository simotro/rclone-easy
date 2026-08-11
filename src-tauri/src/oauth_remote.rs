use crate::rcd::{self, RcdState};
use crate::remotes;
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
struct OAuthUrlEvent {
    url: String,
}

#[derive(Clone, Serialize, PartialEq, Debug)]
struct ExampleOption {
    value: String,
    help: String,
}

#[derive(Clone, Serialize)]
struct OAuthQuestionEvent {
    help: String,
    examples: Vec<ExampleOption>,
    /// Valore suggerito da rclone stesso: usato dalla UI come proposta
    /// precompilata (comunque modificabile), non come risposta automatica.
    #[serde(rename = "defaultValue")]
    default_value: String,
}

/// Domanda del flusso OAuth in attesa di risposta dall'utente (canale
/// `oneshot` usato per sbloccare `run_oauth_flow` una volta arrivata la
/// risposta). Un solo flusso OAuth alla volta nell'app — coerente con
/// `cancel_oauth`, che non prende un nome di remote.
#[derive(Default)]
pub struct PendingOAuthAnswer(tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>);

/// Risposte precompilate per le domande tecniche note poste da rclone
/// prima/dopo l'autorizzazione OAuth vera e propria, indicizzate per
/// `Option.Name` (più stabile dello `State` interno, che è una stringa di
/// implementazione). Le domande non previste qui (es. `choose_type` di
/// OneDrive) non ricevono un default silenzioso: vengono chieste
/// all'utente, vedi `ask_user_for_answer`.
fn scripted_answer(option_name: &str, has_own_client_id: bool) -> Option<&'static str> {
    match option_name {
        "config_is_local" => Some("true"),
        "config_shared_client_id" if !has_own_client_id => Some("true"),
        "config_team_drive" => Some("false"),
        _ => None,
    }
}

/// `client_id`/`client_secret`: quelli creati dall'utente stesso su Google
/// Cloud Console per il proprio remote Drive (vedi il modulo "Usa un client
/// Google tuo" nel form di `nuovo-remote`), non un client condiviso
/// dell'app — Google ritirerà l'accesso all'identità condivisa di rclone
/// nel corso del 2026, quindi ogni utente deve poter portare il proprio.
/// `None`/vuoto = usa ancora l'identità condivisa di rclone (funziona oggi,
/// non per sempre).
fn initial_parameters(client_id: Option<&str>, client_secret: Option<&str>) -> HashMap<String, String> {
    let mut parameters = HashMap::new();
    if let (Some(id), Some(secret)) = (client_id, client_secret) {
        if !id.is_empty() && !secret.is_empty() {
            parameters.insert("client_id".to_string(), id.to_string());
            parameters.insert("client_secret".to_string(), secret.to_string());
        }
    }
    parameters
}

/// Interroga `config/oauthstatus` finché non compare un `authUrl`, poi lo
/// emette come evento per la UI — solo come link di riserva mostrato nel
/// form, cliccabile manualmente se l'apertura automatica non dovesse
/// succedere. Non apre il browser da qui: rclone stesso lo apre già (via
/// `xdg-open`/equivalente) non appena la sua domanda `config_is_local`
/// riceve `true`, cosa che questa app fa sempre — un'apertura nostra in
/// più qui produrrebbe due tab di login per lo stesso OAuth. Gira come task
/// separato, concorrente alla chiamata bloccante di `config/create` che
/// avvia davvero l'attesa OAuth.
fn spawn_oauth_url_watcher<R: tauri::Runtime>(app: AppHandle<R>, info: rcd::ConnectionInfo) {
    tauri::async_runtime::spawn(async move {
        // ~30s a intervalli di 300ms: il tempo che rclone impieghi ad
        // aprire il listener locale è nell'ordine dei millisecondi, questo
        // è già un margine ampio.
        for _ in 0..100 {
            if let Ok(status) = info.call("config/oauthstatus", serde_json::json!({})).await {
                if let Some(url) = status.get("authUrl").and_then(|v| v.as_str()) {
                    let _ = app.emit("rclone-easy://oauth-url", OAuthUrlEvent { url: url.to_string() });
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    });
}

/// Estrae la risposta testuale "grezza" (non JSON-encoded) di un campo
/// `Option`, usando `DefaultStr` invece di `Default`: quest'ultimo è il
/// valore JSON tipizzato (es. `true` senza virgolette per un bool), che
/// `to_string()` renderebbe scorretto per i campi stringa (verrebbe
/// restituito tra virgolette letterali).
fn option_default_str(option: Option<&serde_json::Value>) -> String {
    option.and_then(|o| o.get("DefaultStr")).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

/// Estrae le scelte proposte da rclone per una domanda (es. i tipi di
/// account per `choose_type` di OneDrive), se presenti.
fn examples_from_option(option: Option<&serde_json::Value>) -> Vec<ExampleOption> {
    option
        .and_then(|o| o.get("Examples"))
        .and_then(|v| v.as_array())
        .map(|examples| {
            examples
                .iter()
                .filter_map(|e| {
                    let value = e.get("Value")?.as_str()?.to_string();
                    let help = e.get("Help").and_then(|h| h.as_str()).unwrap_or(&value).to_string();
                    Some(ExampleOption { value, help })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Mette in pausa il flusso e chiede all'utente la risposta a una domanda
/// non prevista dalla tabella precompilata (es. `choose_type` di OneDrive,
/// ma genericamente qualunque domanda imprevista di qualsiasi provider):
/// niente default silenzioso, la scelta spetta all'utente. Salva un canale
/// `oneshot` nello stato condiviso ed emette un evento per la UI; si
/// sblocca quando arriva una risposta (`answer_oauth_question`) o
/// un annullamento (`cancel_oauth`, che fa cadere il canale senza
/// rispondere).
async fn ask_user_for_answer<R: tauri::Runtime>(
    app: &AppHandle<R>,
    pending: &PendingOAuthAnswer,
    option: Option<&serde_json::Value>,
) -> Result<String, String> {
    let help = option.and_then(|o| o.get("Help")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let event = OAuthQuestionEvent {
        help,
        examples: examples_from_option(option),
        default_value: option_default_str(option),
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut guard = pending.0.lock().await;
        *guard = Some(tx);
    }

    let _ = app.emit("rclone-easy://oauth-question", event);

    rx.await.map_err(|_| "autorizzazione annullata dall'utente".to_string())
}

/// Guida il flusso multi-passo di `config/create` per un backend OAuth
/// (drive/dropbox/onedrive) fino al completamento, gestendo il passo di
/// autorizzazione nel browser. Vedi il modulo `remotes.rs` per il gemello
/// non-OAuth (`create_remote_in`).
///
/// `config/create` scrive già una entry (incompleta) al primo passo, ben
/// prima che qualunque domanda sia stata risposta: annullando
/// un'autorizzazione (credenziali mancanti, utente ripensa) a metà del
/// flusso, il nome resterebbe tra i remote propri se si ripulisse solo il
/// fallimento della verifica finale in `verify_and_cleanup`, non un
/// fallimento/annullamento durante il ciclo di domande. Questo wrapper
/// ripulisce con `config/delete` per qualunque errore dopo la creazione
/// iniziale, non solo quello della verifica finale.
async fn run_oauth_flow<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &RcdState,
    pending: &PendingOAuthAnswer,
    name: &str,
    kind: &str,
    extra_answers: &HashMap<String, String>,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<(), String> {
    remotes::ensure_name_available(state, name).await?;

    let result =
        run_oauth_flow_after_name_check(app, state, pending, name, kind, extra_answers, client_id, client_secret)
            .await;

    if result.is_err() {
        let _ = rcd::call(state, "config/delete", serde_json::json!({ "name": name })).await;
    }

    result
}

async fn run_oauth_flow_after_name_check<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &RcdState,
    pending: &PendingOAuthAnswer,
    name: &str,
    kind: &str,
    extra_answers: &HashMap<String, String>,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<(), String> {
    let parameters = initial_parameters(client_id, client_secret);
    let has_own_client_id = !parameters.is_empty();

    let mut response = rcd::call(
        state,
        "config/create",
        serde_json::json!({
            "name": name,
            "type": kind,
            "parameters": parameters,
            "opt": { "nonInteractive": true },
        }),
    )
    .await?;

    loop {
        if let Some(error) = response.get("Error").and_then(|v| v.as_str()) {
            if !error.is_empty() {
                return Err(error.to_string());
            }
        }

        let option_state = response.get("State").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if option_state.is_empty() {
            break; // Nessun'altra domanda: fatto.
        }

        let option = response.get("Option");
        let option_name = option.and_then(|o| o.get("Name")).and_then(|v| v.as_str()).unwrap_or("");

        let answer = if let Some(scripted) = scripted_answer(option_name, has_own_client_id) {
            scripted.to_string()
        } else if let Some(provided) = extra_answers.get(option_name) {
            provided.clone()
        } else {
            // Nessuna risposta precompilata né fornita in anticipo: si
            // chiede all'utente, niente default silenzioso (vedi
            // ask_user_for_answer).
            ask_user_for_answer(app, pending, option).await?
        };

        let continue_body = serde_json::json!({
            "name": name,
            "type": kind,
            "parameters": {},
            "opt": {
                "nonInteractive": true,
                "continue": true,
                "state": option_state,
                "result": answer,
            },
        });

        response = if option_name == "config_is_local" && answer == "true" {
            // La prossima chiamata è quella che si blocca in attesa
            // dell'autorizzazione nel browser: usa connection_info per non
            // tenere il lock su RcdState per tutta l'attesa (congelerebbe
            // ogni altro comando dell'app, vedi rcd::connection_info).
            let info = rcd::connection_info(state).await?;
            spawn_oauth_url_watcher(app.clone(), info.clone());
            info.call("config/create", continue_body).await?
        } else {
            rcd::call(state, "config/create", continue_body).await?
        };
    }

    remotes::verify_and_cleanup(state, name).await
}

/// `client_id`/`client_secret`: presenti solo quando l'utente ha compilato
/// il modulo "Usa un client Google tuo" nel form di `nuovo-remote` (oggi
/// rilevante solo per `kind == "drive"` — Dropbox/OneDrive non sono
/// coinvolti dal ritiro dell'identità condivisa di rclone su Google Drive
/// nel 2026). Entrambi assenti o vuoti: si usa ancora l'identità condivisa.
#[tauri::command]
pub async fn create_oauth_remote(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    pending: tauri::State<'_, PendingOAuthAnswer>,
    name: String,
    kind: String,
    extra_answers: HashMap<String, String>,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<(), String> {
    run_oauth_flow(&app, &state, &pending, &name, &kind, &extra_answers, client_id.as_deref(), client_secret.as_deref())
        .await
}

/// Risposta dell'utente a una domanda del flusso OAuth messa in pausa da
/// `ask_user_for_answer`. Errore esplicito se non c'è nessuna domanda in
/// attesa (es. l'utente ha già annullato, o la finestra dell'evento è
/// arrivata in ritardo).
#[tauri::command]
pub async fn answer_oauth_question(
    pending: tauri::State<'_, PendingOAuthAnswer>,
    answer: String,
) -> Result<(), String> {
    let sender = pending.0.lock().await.take();
    match sender {
        Some(sender) => sender.send(answer).map_err(|_| "il flusso OAuth non è più in attesa".to_string()),
        None => Err("nessuna domanda OAuth in attesa di risposta".to_string()),
    }
}

/// Annulla un'autorizzazione OAuth in corso: se c'è una domanda in attesa di
/// risposta (il vero browser non è ancora partito), la lascia cadere senza
/// rispondere — `ask_user_for_answer` si sblocca con un errore pulito.
/// Altrimenti prova a interrompere l'attesa nel browser con
/// `config/oauthstop` (verificato in fase di investigazione: fa fallire con
/// un errore pulito la chiamata `config/create` bloccata). Se non c'è
/// nessuna autorizzazione in corso (es. l'utente annulla dopo che è già
/// finita), rclone risponde con un proprio errore che qui viene ignorato:
/// non è un problema dell'app.
#[tauri::command]
pub async fn cancel_oauth(
    state: tauri::State<'_, RcdState>,
    pending: tauri::State<'_, PendingOAuthAnswer>,
) -> Result<(), String> {
    let had_pending_question = pending.0.lock().await.take().is_some();
    if !had_pending_question {
        let _ = rcd::call(&state, "config/oauthstop", serde_json::json!({})).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::Manager;

    #[test]
    fn scripted_answer_always_says_yes_to_local_browser() {
        assert_eq!(scripted_answer("config_is_local", false), Some("true"));
        assert_eq!(scripted_answer("config_is_local", true), Some("true"));
    }

    #[test]
    fn scripted_answer_accepts_shared_client_id_only_without_a_custom_one() {
        assert_eq!(scripted_answer("config_shared_client_id", false), Some("true"));
        assert_eq!(scripted_answer("config_shared_client_id", true), None);
    }

    #[test]
    fn scripted_answer_declines_team_drive() {
        assert_eq!(scripted_answer("config_team_drive", false), Some("false"));
    }

    #[test]
    fn scripted_answer_is_none_for_unknown_questions() {
        assert_eq!(scripted_answer("choose_type", false), None);
        assert_eq!(scripted_answer("qualcosa_di_mai_visto", false), None);
    }

    #[test]
    fn initial_parameters_are_empty_without_a_custom_client_id() {
        assert!(initial_parameters(None, None).is_empty());
        assert!(initial_parameters(Some(""), Some("")).is_empty());
        assert!(initial_parameters(Some("id"), None).is_empty(), "serve sia client_id sia client_secret, non uno solo");
    }

    #[test]
    fn initial_parameters_are_set_when_both_client_id_and_secret_are_given() {
        let parameters = initial_parameters(Some("un-client-id"), Some("un-client-secret"));
        assert_eq!(parameters.get("client_id"), Some(&"un-client-id".to_string()));
        assert_eq!(parameters.get("client_secret"), Some(&"un-client-secret".to_string()));
    }

    #[test]
    fn option_default_str_reads_the_plain_string_field() {
        let option = serde_json::json!({"DefaultStr": "false", "Default": false});
        assert_eq!(option_default_str(Some(&option)), "false");
    }

    #[test]
    fn option_default_str_is_empty_when_missing() {
        assert_eq!(option_default_str(None), "");
    }

    #[tokio::test]
    async fn run_oauth_flow_fails_cleanly_for_an_unknown_backend_type() {
        let dir = crate::rcd::tests::TempDir::new("oauth-unknown-type");
        let state = rcd::build_state(dir.config_path()).await;

        // Nessuna AppHandle reale disponibile nei test unitari: qui basta
        // verificare che un tipo di backend inesistente fallisca subito al
        // primo config/create, prima ancora di arrivare a usare `app`.
        // tauri::test::mock_app() fornisce un AppHandle utilizzabile.
        let mock = tauri::test::mock_app();
        let app = mock.handle().clone();

        let pending = PendingOAuthAnswer::default();
        let result =
            run_oauth_flow(&app, &state, &pending, "oauth-test", "tipo-che-non-esiste", &HashMap::new(), None, None).await;
        assert!(result.is_err());
    }

    /// Prova end-to-end (senza completare un login reale, non automatizzabile
    /// con dati finti) di due cose insieme: che il ciclo arrivi davvero al
    /// passo di attesa OAuth per un provider reale (dropbox, il più
    /// semplice), e che durante l'attesa il lock su RcdState sia
    /// effettivamente libero — se non lo fosse, le chiamate concorrenti a
    /// config/oauthstatus qui sotto si bloccherebbero e il test andrebbe in
    /// timeout invece di trovare un authUrl.
    #[tokio::test]
    async fn run_oauth_flow_reaches_oauth_wait_without_freezing_other_calls_and_can_be_cancelled() {
        let dir = crate::rcd::tests::TempDir::new("oauth-dropbox-cancel");
        let state = std::sync::Arc::new(rcd::build_state(dir.config_path()).await);
        let mock = tauri::test::mock_app();
        let app = mock.handle().clone();

        let flow_state = state.clone();
        let pending = PendingOAuthAnswer::default();
        let flow_task = tokio::spawn(async move {
            run_oauth_flow(&app, &flow_state, &pending, "dbx-cancel-test", "dropbox", &HashMap::new(), None, None).await
        });

        let mut auth_url_seen = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Ok(body) = rcd::call(&state, "config/oauthstatus", serde_json::json!({})).await {
                if body.get("authUrl").and_then(|v| v.as_str()).is_some() {
                    auth_url_seen = true;
                    break;
                }
            }
        }
        assert!(auth_url_seen, "atteso un authUrl entro il timeout: il lock potrebbe non essere stato rilasciato");

        let _ = rcd::call(&state, "config/oauthstop", serde_json::json!({})).await;

        let outcome = tokio::time::timeout(Duration::from_secs(10), flow_task).await;
        assert!(outcome.is_ok(), "run_oauth_flow dovrebbe terminare dopo l'annullamento, non restare bloccato");
        assert!(outcome.unwrap().unwrap().is_err(), "il flusso annullato deve restituire un errore");

        // config/create scrive già una entry al primissimo passo, ben prima
        // che qualunque domanda sia stata risposta — un annullamento a metà
        // flusso non deve lasciarla lì.
        let listremotes = rcd::call(&state, "config/listremotes", serde_json::json!({})).await.unwrap();
        let remaining: Vec<&str> =
            listremotes.get("remotes").and_then(|v| v.as_array()).unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert!(
            !remaining.contains(&"dbx-cancel-test"),
            "il remote non deve restare salvato dopo un annullamento a metà flusso: {remaining:?}"
        );
    }

    #[test]
    fn examples_from_option_reads_value_and_help() {
        let option = serde_json::json!({
            "Examples": [
                {"Value": "onedrive", "Help": "Account personale"},
                {"Value": "onedrive_business", "Help": "Account aziendale o SharePoint"}
            ]
        });
        assert_eq!(
            examples_from_option(Some(&option)),
            vec![
                ExampleOption { value: "onedrive".to_string(), help: "Account personale".to_string() },
                ExampleOption {
                    value: "onedrive_business".to_string(),
                    help: "Account aziendale o SharePoint".to_string()
                },
            ]
        );
    }

    #[test]
    fn examples_from_option_is_empty_when_missing() {
        assert_eq!(examples_from_option(None), Vec::new());
        assert_eq!(examples_from_option(Some(&serde_json::json!({}))), Vec::new());
    }

    /// Verifica il meccanismo di pausa/risposta usando direttamente il
    /// comando `answer_oauth_question`, non solo la logica interna — prova
    /// end-to-end che una domanda non prevista si sblocca con la risposta
    /// scelta dall'utente, senza alcun default silenzioso.
    #[tokio::test]
    async fn answer_oauth_question_resolves_a_pending_ask() {
        let mock = tauri::test::mock_app();
        let app = mock.handle().clone();
        app.manage(PendingOAuthAnswer::default());

        let option = serde_json::json!({
            "Help": "Che tipo di account OneDrive?",
            "Examples": [
                {"Value": "onedrive", "Help": "Account personale"},
                {"Value": "onedrive_business", "Help": "Account aziendale o SharePoint"}
            ]
        });

        let ask_app = app.clone();
        let ask_task = tokio::spawn(async move {
            let pending = ask_app.state::<PendingOAuthAnswer>();
            ask_user_for_answer(&ask_app, &pending, Some(&option)).await
        });

        // Aspetta che ask_user_for_answer abbia registrato il sender prima
        // di rispondere.
        for _ in 0..50 {
            if app.state::<PendingOAuthAnswer>().0.lock().await.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        answer_oauth_question(app.state::<PendingOAuthAnswer>(), "onedrive_business".to_string())
            .await
            .expect("dovrebbe esserci una domanda in attesa");

        assert_eq!(ask_task.await.unwrap().unwrap(), "onedrive_business");
    }

    #[tokio::test]
    async fn answer_oauth_question_fails_when_nothing_is_pending() {
        let mock = tauri::test::mock_app();
        let app = mock.handle().clone();
        app.manage(PendingOAuthAnswer::default());

        let result = answer_oauth_question(app.state::<PendingOAuthAnswer>(), "qualsiasi".to_string()).await;
        assert!(result.is_err());
    }

    /// Prova che l'annullamento (stessa logica di `cancel_oauth`: prelevare
    /// e far cadere il sender senza rispondere) sblocca la domanda in attesa
    /// con un errore pulito, invece di lasciarla bloccata per sempre.
    #[tokio::test]
    async fn ask_user_for_answer_fails_cleanly_when_the_sender_is_dropped() {
        let mock = tauri::test::mock_app();
        let app = mock.handle().clone();
        app.manage(PendingOAuthAnswer::default());

        let option = serde_json::json!({"Help": "una domanda qualsiasi"});

        let ask_app = app.clone();
        let ask_task = tokio::spawn(async move {
            let pending = ask_app.state::<PendingOAuthAnswer>();
            ask_user_for_answer(&ask_app, &pending, Some(&option)).await
        });

        for _ in 0..50 {
            if app.state::<PendingOAuthAnswer>().0.lock().await.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let _ = app.state::<PendingOAuthAnswer>().0.lock().await.take();

        assert!(ask_task.await.unwrap().is_err());
    }
}
