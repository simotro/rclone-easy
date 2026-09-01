use crate::rcd::{self, RcdState};
use crate::remotes;
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Guida `config/create` fino al completamento per un backend qualsiasi, non
/// solo quelli OAuth: verificato che rclone risponde con una domanda
/// (`State`/`Option` non vuoti) solo per i pochi backend con un passaggio
/// interattivo incorporato nel proprio codice (l'autorizzazione OAuth è il
/// caso principale) — per tutti gli altri, `config/create` con
/// `nonInteractive: true` accetta silenziosamente anche parametri mancanti o
/// vuoti, comprese le opzioni segnate `Required` nel loro schema, senza mai
/// fare domande. Per questo il form generico (`remotes.rs::get_provider_options`)
/// raccoglie prima tutti i campi noti lato frontend e li manda in un colpo
/// solo qui: se il backend non ha bisogno d'altro, `config/create` finisce
/// subito; se invece ha un passaggio speciale (OAuth), il ciclo sotto lo
/// gestisce esattamente come già faceva per drive/dropbox/onedrive.

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
///
/// `default_to_my_drive`: `false` solo per il flusso "Drive condiviso"
/// (`create_shared_drive_remote`) — lascia passare `config_team_drive`
/// invece di rispondere sempre "false", così arriva fino a
/// `ask_user_for_answer` con l'elenco reale degli Shared Drive
/// dell'account (rclone lo popola da sé in `Examples`, verificato dal
/// vivo) mostrato come scelta, esattamente come già succede oggi per
/// `choose_type` di OneDrive — nessuna UI nuova serve per quella parte.
fn scripted_answer(option_name: &str, has_own_client_id: bool, default_to_my_drive: bool) -> Option<&'static str> {
    match option_name {
        "config_is_local" => Some("true"),
        "config_shared_client_id" if !has_own_client_id => Some("true"),
        "config_team_drive" if default_to_my_drive => Some("false"),
        _ => None,
    }
}

/// Risposte automatiche aggiuntive del percorso guidato (provider curati) —
/// applicate solo lì, mai in "Configurazione avanzata": lì l'utente deve
/// vedere ogni domanda reale di rclone, senza scorciatoie, per restare
/// fedele alla promessa "come rclone config, passo per passo". Il confronto
/// è sul testo della scelta (`Help`), non su un valore interno di rclone:
/// non verificabile senza completare un vero OAuth (serve un login reale nel
/// browser), il testo mostrato all'utente durante l'indagine era invece già
/// noto con certezza.
///
/// `config_type` (non `choose_type`, che è invece il nome dello *state*
/// interno di rclone per questo passo — campo diverso, verificato nel
/// sorgente di `backend/onedrive/onedrive.go`:
/// `fs.ConfigChooseExclusiveFixed("choose_type_done", "config_type", "Type
/// of connection", ...)`) è il nome reale dell'`Option` restituita da
/// rclone per la domanda "Type of connection".
fn guided_answer(option_name: &str, kind: &str, examples: &[ExampleOption]) -> Option<String> {
    if kind == "onedrive" && option_name == "config_type" {
        return examples
            .iter()
            .find(|e| {
                let help = e.help.to_lowercase();
                help.contains("personal") && help.contains("business")
            })
            .map(|e| e.value.clone());
    }
    None
}

/// `true` se il chiamante ha fornito un proprio `client_id`/`client_secret`
/// (tipicamente creati dall'utente su Google Cloud Console per il proprio
/// remote Drive, vedi il modulo "Usa un client Google tuo" nel form di
/// `nuovo-remote` — Google ritirerà l'identità condivisa di rclone nel corso
/// del 2026, quindi ogni utente deve poter portare il proprio) — governa solo
/// la risposta scriptata a `config_shared_client_id` sotto, il resto dei
/// parametri passa comunque per intero a `config/create`.
fn has_own_client_id(parameters: &HashMap<String, String>) -> bool {
    !parameters.get("client_id").is_none_or(String::is_empty) && !parameters.get("client_secret").is_none_or(String::is_empty)
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

/// Comportamenti del ciclo che dipendono da CHI sta chiamando, non dal tipo
/// di backend — raggruppati qui invece che come parametri booleani separati
/// (facili da scambiare d'ordine man mano che se ne aggiungono).
#[derive(Clone, Copy)]
struct FlowOptions {
    /// `true` solo nel percorso guidato (provider curati) — vedi
    /// `guided_answer`, mai nella "Configurazione avanzata" né nel riuso
    /// credenziali per uno Shared Drive.
    guided: bool,
    /// Vedi il commento su `scripted_answer`.
    default_to_my_drive: bool,
    /// `true` se `parameters` contiene valori già offuscati, riusati da un
    /// remote esistente (`create_shared_drive_remote`) — evita di
    /// offuscarli una seconda volta, corromperebbe il segreto (stesso
    /// principio di `remotes::create_remote_in`).
    already_obscured: bool,
}

/// Guida il flusso multi-passo di `config/create` per un backend qualsiasi
/// fino al completamento, gestendo anche l'eventuale passo di autorizzazione
/// nel browser se il backend lo richiede. Vedi `remotes.rs::create_remote_in`
/// per il gemello a un solo colpo, usato quando tutti i parametri sono già
/// noti in anticipo e non serve passare dal ciclo di domande.
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
    parameters: &HashMap<String, String>,
    options: FlowOptions,
) -> Result<(), String> {
    remotes::ensure_name_available(state, name).await?;

    let result = run_oauth_flow_after_name_check(app, state, pending, name, kind, extra_answers, parameters, options).await;

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
    parameters: &HashMap<String, String>,
    options: FlowOptions,
) -> Result<(), String> {
    let has_own_client_id = has_own_client_id(parameters);

    let mut response = rcd::call(
        state,
        "config/create",
        serde_json::json!({
            "name": name,
            "type": kind,
            "parameters": parameters,
            "opt": { "nonInteractive": true, "obscure": !options.already_obscured, "noObscure": options.already_obscured },
        }),
    )
    .await?;

    loop {
        if let Some(error) = response.get("Error").and_then(|v| v.as_str()) {
            if !error.is_empty() {
                // Solo nel percorso guidato: rclone stessa non offre un
                // modo alternativo di procedere qui (l'errore chiude il
                // flusso, non è un'altra domanda a cui rispondere
                // diversamente) — vedi `onedrive_recovery` per il perché
                // serve parlare direttamente con Microsoft Graph.
                if options.guided && kind == "onedrive" && crate::onedrive_recovery::is_drive_listing_failure(error) {
                    match crate::onedrive_recovery::try_recover_drive_id(state, name).await {
                        Ok(()) => break,
                        Err(recovery_error) => {
                            return Err(format!("{error} (recupero automatico anch'esso non riuscito: {recovery_error})"));
                        }
                    }
                }
                return Err(error.to_string());
            }
        }

        let option_state = response.get("State").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if option_state.is_empty() {
            break; // Nessun'altra domanda: fatto.
        }

        let option = response.get("Option");
        let option_name = option.and_then(|o| o.get("Name")).and_then(|v| v.as_str()).unwrap_or("");

        let answer = if let Some(scripted) = scripted_answer(option_name, has_own_client_id, options.default_to_my_drive) {
            scripted.to_string()
        } else if let Some(provided) = extra_answers.get(option_name) {
            provided.clone()
        } else if options.guided {
            if let Some(guided) = guided_answer(option_name, kind, &examples_from_option(option)) {
                guided
            } else {
                ask_user_for_answer(app, pending, option).await?
            }
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

    remotes::verify_and_cleanup(state, name, kind, "").await
}

/// Crea un remote di qualunque tipo passando dal ciclo interattivo di
/// `config/create` — `parameters` sono i campi già noti (raccolti dal form
/// generico lato frontend, es. `client_id`/`client_secret` per un client
/// Google proprio, o qualunque altro campo statico del backend scelto): se
/// bastano, il remote si crea subito; se il backend ha un passaggio speciale
/// in più (l'OAuth è il caso principale), il resto del ciclo lo gestisce
/// come sempre.
#[tauri::command]
pub async fn create_remote_interactive(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    pending: tauri::State<'_, PendingOAuthAnswer>,
    name: String,
    kind: String,
    extra_answers: HashMap<String, String>,
    parameters: HashMap<String, String>,
    guided: bool,
) -> Result<(), String> {
    let options = FlowOptions { guided, default_to_my_drive: true, already_obscured: false };
    run_oauth_flow(&app, &state, &pending, &name, &kind, &extra_answers, &parameters, options).await
}

/// Crea un remote Google Drive puntato su uno Shared Drive (Team Drive)
/// invece che su "Il mio Drive", riusando client_id/client_secret/token di
/// un remote Drive già configurato — nessuna nuova autorizzazione nel
/// browser. `source_remote_name` deve essere un remote esistente di tipo
/// `drive`; i suoi parametri (comprese le credenziali, già offuscate così
/// come salvate) sono letti qui e mai esposti al frontend, stesso principio
/// di `remotes::rename_remote_in`.
///
/// La scelta di QUALE Shared Drive passa dal ciclo standard sopra:
/// `extra_answers` risponde "no" a "sostituire il token?" (lo si vuole
/// riusare, non rinnovare) e "sì" a "configuralo come Shared Drive?" —
/// dopodiché rclone stessa restituisce l'elenco reale degli Shared Drive
/// visibili per l'account come domanda a scelta multipla (`config_team_drive`,
/// verificato dal vivo), che arriva alla UI attraverso lo stesso meccanismo
/// generico già usato per `choose_type` di OneDrive: nessuna lista
/// costruita a mano qui.
#[tauri::command]
pub async fn create_shared_drive_remote(
    app: AppHandle,
    state: tauri::State<'_, RcdState>,
    pending: tauri::State<'_, PendingOAuthAnswer>,
    new_name: String,
    source_remote_name: String,
) -> Result<(), String> {
    create_shared_drive_remote_for(&app, &state, &pending, &new_name, &source_remote_name).await
}

/// Corpo di `create_shared_drive_remote`, generico su `R: tauri::Runtime`
/// come `run_oauth_flow` — separato dal comando Tauri (che vuole
/// `AppHandle` concreto, non testabile con `tauri::test::mock_app()`) solo
/// per poterlo chiamare direttamente nei test.
async fn create_shared_drive_remote_for<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &RcdState,
    pending: &PendingOAuthAnswer,
    new_name: &str,
    source_remote_name: &str,
) -> Result<(), String> {
    let dump = rcd::call(state, "config/dump", serde_json::json!({})).await?;
    let (kind, source_parameters) = crate::existing_config::extract_remote_parameters(&dump, source_remote_name)?;
    if kind != "drive" {
        return Err(format!("'{source_remote_name}' non è un remote Google Drive"));
    }

    let parameters: HashMap<String, String> = ["client_id", "client_secret", "token"]
        .into_iter()
        .filter_map(|key| source_parameters.get(key).map(|value| (key.to_string(), value.clone())))
        .collect();
    let extra_answers = HashMap::from([
        ("config_refresh_token".to_string(), "false".to_string()),
        ("config_change_team_drive".to_string(), "true".to_string()),
    ]);
    let options = FlowOptions { guided: false, default_to_my_drive: false, already_obscured: true };

    run_oauth_flow(app, state, pending, new_name, "drive", &extra_answers, &parameters, options).await
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
        assert_eq!(scripted_answer("config_is_local", false, true), Some("true"));
        assert_eq!(scripted_answer("config_is_local", true, true), Some("true"));
    }

    #[test]
    fn scripted_answer_accepts_shared_client_id_only_without_a_custom_one() {
        assert_eq!(scripted_answer("config_shared_client_id", false, true), Some("true"));
        assert_eq!(scripted_answer("config_shared_client_id", true, true), None);
    }

    #[test]
    fn scripted_answer_declines_team_drive_by_default() {
        assert_eq!(scripted_answer("config_team_drive", false, true), Some("false"));
    }

    /// Il flusso "Drive condiviso" (`create_shared_drive_remote`) passa
    /// `default_to_my_drive: false` apposta per NON rispondere qui — la
    /// domanda deve arrivare all'utente con l'elenco reale degli Shared
    /// Drive, vedi il commento su `scripted_answer`.
    #[test]
    fn scripted_answer_lets_team_drive_through_when_not_defaulting_to_my_drive() {
        assert_eq!(scripted_answer("config_team_drive", false, false), None);
    }

    #[test]
    fn scripted_answer_is_none_for_unknown_questions() {
        assert_eq!(scripted_answer("config_type", false, true), None);
        assert_eq!(scripted_answer("qualcosa_di_mai_visto", false, true), None);
    }

    #[test]
    fn guided_answer_picks_the_personal_or_business_example_for_onedrive() {
        let examples = vec![
            ExampleOption { value: "onedrive".to_string(), help: "OneDrive Personal or Business".to_string() },
            ExampleOption { value: "sharepoint".to_string(), help: "Root Sharepoint site".to_string() },
        ];
        assert_eq!(guided_answer("config_type", "onedrive", &examples), Some("onedrive".to_string()));
    }

    #[test]
    fn guided_answer_is_none_outside_onedrives_choose_type_question() {
        let examples = vec![ExampleOption { value: "onedrive".to_string(), help: "OneDrive Personal or Business".to_string() }];
        assert_eq!(guided_answer("some_other_question", "onedrive", &examples), None);
        assert_eq!(guided_answer("config_type", "dropbox", &examples), None);
    }

    #[test]
    fn guided_answer_is_none_if_no_example_mentions_both_personal_and_business() {
        let examples = vec![ExampleOption { value: "sharepoint".to_string(), help: "Root Sharepoint site".to_string() }];
        assert_eq!(guided_answer("config_type", "onedrive", &examples), None);
    }

    #[test]
    fn has_own_client_id_is_false_without_both_fields() {
        assert!(!has_own_client_id(&HashMap::new()));
        assert!(!has_own_client_id(&HashMap::from([("client_id".to_string(), "".to_string())])));
        assert!(!has_own_client_id(&HashMap::from([("client_id".to_string(), "id".to_string())])), "serve anche client_secret, non uno solo");
    }

    #[test]
    fn has_own_client_id_is_true_when_both_are_set() {
        let parameters = HashMap::from([
            ("client_id".to_string(), "un-client-id".to_string()),
            ("client_secret".to_string(), "un-client-secret".to_string()),
        ]);
        assert!(has_own_client_id(&parameters));
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
        let options = FlowOptions { guided: false, default_to_my_drive: true, already_obscured: false };
        let result =
            run_oauth_flow(&app, &state, &pending, "oauth-test", "tipo-che-non-esiste", &HashMap::new(), &HashMap::new(), options).await;
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
            let options = FlowOptions { guided: false, default_to_my_drive: true, already_obscured: false };
            run_oauth_flow(&app, &flow_state, &pending, "dbx-cancel-test", "dropbox", &HashMap::new(), &HashMap::new(), options).await
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

    #[tokio::test]
    async fn create_shared_drive_remote_rejects_a_source_that_is_not_a_drive_remote() {
        let dir = crate::rcd::tests::TempDir::new("shared-drive-wrong-kind");
        let state = rcd::build_state(dir.config_path()).await;
        let mock = tauri::test::mock_app();
        let app = mock.handle().clone();
        let pending = PendingOAuthAnswer::default();

        // Un remote locale, tipo qualunque tranne "drive": nessuna
        // autorizzazione OAuth necessaria per crearlo nel test.
        let options = FlowOptions { guided: false, default_to_my_drive: true, already_obscured: false };
        run_oauth_flow(&app, &state, &pending, "non-drive", "local", &HashMap::new(), &HashMap::new(), options).await.unwrap();

        let new_pending = PendingOAuthAnswer::default();
        let result = create_shared_drive_remote_for(&app, &state, &new_pending, "nuovo", "non-drive").await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("non è un remote Google Drive"));
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
