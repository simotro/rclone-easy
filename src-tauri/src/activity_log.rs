//! Registro delle attività di tutti i remote: un evento per ogni file
//! copiato, cancellato, spostato o andato in errore, più gli avvisi e gli
//! esiti dei mount. Alimenta la pagina "Attività".
//!
//! Formato: una riga JSON per evento in `activity.jsonl`, in ordine
//! cronologico (la più recente in fondo). Gli eventi si scrivono a fine run,
//! non durante. La rotazione tiene al più `RETENTION_DAYS` giorni e
//! `MAX_EVENTS` eventi, perché un resync su un albero grande ne produce
//! migliaia in un colpo solo.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const FILE_NAME: &str = "activity.jsonl";
const RETENTION_DAYS: u64 = 30;
const MAX_EVENTS: usize = 20_000;
/// Righe restituite al massimo per pagina.
const MAX_PAGE: usize = 500;

/// Serializza scritture e letture del file: più job possono finire nello
/// stesso istante.
fn file_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Un evento del registro.
///
/// `kind`: `copied`, `deleted`, `moved`, `conflict`, `warning`, `error`,
/// `mounted`, `unmounted`.
/// `direction`: `upload`, `download`, `sync` oppure vuoto se non applicabile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub when_unix: u64,
    pub remote: String,
    /// `backup`, `bisync` o `mount`.
    pub service: String,
    pub job: String,
    pub kind: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub message: String,
}

impl ActivityEvent {
    pub(crate) fn new(service: &str, job: &str, remote: &str, kind: &str) -> Self {
        ActivityEvent {
            when_unix: now_unix(),
            remote: remote.to_string(),
            service: service.to_string(),
            job: job.to_string(),
            kind: kind.to_string(),
            direction: String::new(),
            path: String::new(),
            size: None,
            message: String::new(),
        }
    }

    pub(crate) fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    pub(crate) fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub(crate) fn with_direction(mut self, direction: &str) -> Self {
        self.direction = direction.to_string();
        self
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn log_path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE_NAME)
}

fn read_events(config_dir: &Path) -> Vec<ActivityEvent> {
    let Ok(content) = std::fs::read_to_string(log_path(config_dir)) else { return Vec::new() };
    content.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

fn write_events(config_dir: &Path, events: &[ActivityEvent]) -> Result<(), String> {
    let mut content = String::new();
    for event in events {
        content.push_str(&serde_json::to_string(event).map_err(|e| format!("impossibile serializzare l'evento: {e}"))?);
        content.push('\n');
    }
    crate::fs_atomic::write_atomically(&log_path(config_dir), &content)
}

/// Toglie gli eventi più vecchi della finestra di conservazione e, se ancora
/// troppi, i più vecchi in eccesso.
fn prune(events: &mut Vec<ActivityEvent>, now: u64) {
    let cutoff = now.saturating_sub(RETENTION_DAYS * 24 * 60 * 60);
    events.retain(|e| e.when_unix >= cutoff);
    if events.len() > MAX_EVENTS {
        events.drain(..events.len() - MAX_EVENTS);
    }
}

/// Aggiunge `new_events` in coda al registro e applica la rotazione.
/// Best-effort: un errore qui non deve far fallire il run che l'ha prodotto,
/// quindi il chiamante lo ignora.
pub(crate) fn append_events(config_dir: &Path, new_events: Vec<ActivityEvent>) -> Result<(), String> {
    if new_events.is_empty() {
        return Ok(());
    }
    let _guard = file_lock().lock().unwrap();
    let mut events = read_events(config_dir);
    events.extend(new_events);
    prune(&mut events, now_unix());
    write_events(config_dir, &events)
}

/// Filtri della pagina Attività. `kind` è un gruppo di tipi: `all`,
/// `warnings` (avvisi e conflitti), `errors`, `copied`, `deleted`, `moved`,
/// `mounts`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityFilter {
    pub remote: Option<String>,
    pub kind: Option<String>,
    pub search: Option<String>,
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPage {
    /// Dal più recente al meno recente.
    pub events: Vec<ActivityEvent>,
    /// Quanti eventi corrispondono ai filtri in tutto, non solo in questa
    /// pagina.
    pub total: usize,
    /// Remote che compaiono nel registro, per popolare il filtro.
    pub remotes: Vec<String>,
}

fn matches_kind(group: &str, kind: &str) -> bool {
    match group {
        "warnings" => kind == "warning" || kind == "conflict",
        "errors" => kind == "error",
        "copied" => kind == "copied",
        "deleted" => kind == "deleted",
        "moved" => kind == "moved",
        "mounts" => kind == "mounted" || kind == "unmounted",
        _ => true,
    }
}

pub(crate) fn query(config_dir: &Path, filter: &ActivityFilter) -> ActivityPage {
    let _guard = file_lock().lock().unwrap();
    let events = read_events(config_dir);

    let mut remotes: Vec<String> = events.iter().map(|e| e.remote.clone()).collect();
    remotes.sort();
    remotes.dedup();

    let search = filter.search.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_lowercase);
    let group = filter.kind.as_deref().unwrap_or("all");
    let matching: Vec<&ActivityEvent> = events
        .iter()
        .rev()
        .filter(|e| filter.remote.as_deref().is_none_or(|r| r.is_empty() || e.remote == r))
        .filter(|e| matches_kind(group, &e.kind))
        .filter(|e| match &search {
            Some(needle) => e.path.to_lowercase().contains(needle) || e.message.to_lowercase().contains(needle),
            None => true,
        })
        .collect();

    let limit = filter.limit.unwrap_or(200).min(MAX_PAGE);
    ActivityPage {
        total: matching.len(),
        events: matching.into_iter().skip(filter.offset).take(limit).cloned().collect(),
        remotes,
    }
}

#[tauri::command]
pub fn list_activity(app: AppHandle, filter: ActivityFilter) -> Result<ActivityPage, String> {
    let config_dir =
        app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))?;
    Ok(query(&config_dir, &filter))
}

/// Svuota il registro — azione esplicita dell'utente dalla pagina Attività.
#[tauri::command]
pub fn clear_activity(app: AppHandle) -> Result<(), String> {
    let config_dir =
        app.path().app_config_dir().map_err(|e| format!("impossibile determinare la cartella di configurazione: {e}"))?;
    let _guard = file_lock().lock().unwrap();
    write_events(&config_dir, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;

    fn event(remote: &str, kind: &str, path: &str, when: u64) -> ActivityEvent {
        let mut e = ActivityEvent::new("bisync", "job", remote, kind).with_path(path);
        e.when_unix = when;
        e
    }

    #[test]
    fn events_are_returned_newest_first_and_filtered() {
        let dir = TempDir::new("activity-filter");
        std::fs::create_dir_all(&dir.path).unwrap();
        let now = now_unix();
        append_events(
            &dir.path,
            vec![
                event("Drive", "copied", "a/uno.txt", now - 3),
                event("Mega", "error", "b/due.txt", now - 2),
                event("Drive", "warning", "", now - 1),
                event("Drive", "deleted", "a/Tre.txt", now),
            ],
        )
        .unwrap();

        let all = query(&dir.path, &ActivityFilter::default());
        assert_eq!(all.total, 4);
        assert_eq!(all.events[0].path, "a/Tre.txt", "il più recente per primo");
        assert_eq!(all.remotes, vec!["Drive".to_string(), "Mega".to_string()]);

        let drive = query(&dir.path, &ActivityFilter { remote: Some("Drive".into()), ..Default::default() });
        assert_eq!(drive.total, 3);

        let errors = query(&dir.path, &ActivityFilter { kind: Some("errors".into()), ..Default::default() });
        assert_eq!(errors.events.len(), 1);
        assert_eq!(errors.events[0].remote, "Mega");

        let warnings = query(&dir.path, &ActivityFilter { kind: Some("warnings".into()), ..Default::default() });
        assert_eq!(warnings.total, 1);

        let search = query(&dir.path, &ActivityFilter { search: Some("TRE".into()), ..Default::default() });
        assert_eq!(search.total, 1, "la ricerca non distingue maiuscole");
    }

    #[test]
    fn paging_skips_and_limits_but_reports_the_full_total() {
        let dir = TempDir::new("activity-paging");
        std::fs::create_dir_all(&dir.path).unwrap();
        let now = now_unix();
        append_events(&dir.path, (0..10).map(|i| event("Drive", "copied", &format!("f{i}"), now)).collect()).unwrap();
        let page = query(&dir.path, &ActivityFilter { offset: 2, limit: Some(3), ..Default::default() });
        assert_eq!(page.total, 10);
        assert_eq!(page.events.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), vec!["f7", "f6", "f5"]);
    }

    #[test]
    fn old_events_and_events_beyond_the_cap_are_rotated_out() {
        let now = now_unix();
        let mut events = vec![event("Drive", "copied", "vecchio", now - (RETENTION_DAYS + 1) * 86_400)];
        events.extend((0..MAX_EVENTS + 5).map(|i| event("Drive", "copied", &format!("f{i}"), now)));
        prune(&mut events, now);
        assert_eq!(events.len(), MAX_EVENTS);
        assert!(events.iter().all(|e| e.path != "vecchio"));
        assert_eq!(events[0].path, "f5", "restano i più recenti");
    }

    #[test]
    fn a_corrupted_line_is_skipped_instead_of_losing_the_log() {
        let dir = TempDir::new("activity-corrupt");
        std::fs::create_dir_all(&dir.path).unwrap();
        append_events(&dir.path, vec![event("Drive", "copied", "ok", now_unix())]).unwrap();
        let path = log_path(&dir.path);
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str("{non json\n");
        std::fs::write(&path, content).unwrap();
        assert_eq!(query(&dir.path, &ActivityFilter::default()).total, 1);
    }
}
