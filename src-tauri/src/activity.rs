use crate::rcd::RcdState;
use std::path::Path;

/// Uno dei tre servizi che si possono configurare per un remote. Al più
/// uno alla volta può essere "attivo" (vedi `ensure_no_other_active_service`),
/// per evitare i rischi di avere, per esempio, mount e bisync attivi insieme
/// sullo stesso remote (bisync confronterebbe il remote con se stesso
/// attraverso due percorsi diversi).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceKind {
    Mount,
    Backup,
    Bisync,
}

impl ServiceKind {
    fn label(self) -> &'static str {
        match self {
            ServiceKind::Mount => "il mount",
            ServiceKind::Backup => "il backup",
            ServiceKind::Bisync => "la sincronizzazione bidirezionale",
        }
    }
}

/// Nome del remote referenziato da una stringa `fs` (`remoto:percorso`),
/// `None` se è un percorso locale — riconosciuto tramite `Path::is_absolute`
/// (tutti i percorsi locali scelti dal selettore di cartelle sono assoluti,
/// nessun nome di remote rclone lo è: su Windows un `C:\...` è assoluto ma
/// `starts_with('/')` non lo riconoscerebbe, scambiandolo per un remote
/// chiamato "C" — bug reale osservato in tray.rs, corretto qui allo stesso
/// modo). Stessa funzione duplicata in `mounts.rs`/`jobs.rs`/`bisync.rs`/
/// `tray.rs`: qui serve una copia propria per non introdurre una dipendenza
/// incrociata tra quei moduli.
fn remote_name_of(fs: &str) -> Option<&str> {
    if Path::new(fs).is_absolute() {
        return None;
    }
    fs.split_once(':').map(|(name, _)| name)
}

/// Quale servizio, se non nessuno, è attualmente "attivo" per un remote:
/// mount = montato ora (stato live via `mount/listmounts`) OPPURE
/// configurato con `auto_mount: true` (si monterebbe da solo al prossimo
/// avvio dell'app, quindi conta come "attivo" già da ora — altrimenti si
/// potrebbe attivare un backup automatico sullo stesso remote nella
/// finestra tra "auto_mount impostato" e "l'app viene riavviata e monta
/// davvero", bypassando la mutua esclusività); backup/bisync =
/// sincronizzazione automatica accesa (`autoIntervalMinutes` impostato,
/// non solo "configurato"). Un servizio configurato ma non attivo (mount
/// smontato e senza auto_mount, backup/bisync senza intervallo automatico)
/// non conta.
pub(crate) async fn active_service_for_remote(
    state: &RcdState,
    config_dir: &Path,
    remote_name: &str,
) -> Result<Option<ServiceKind>, String> {
    let mounts = crate::mounts::load_from_dir(config_dir).unwrap_or_default();
    if let Some(m) = mounts.iter().find(|m| remote_name_of(&m.remote) == Some(remote_name)) {
        if m.auto_mount {
            return Ok(Some(ServiceKind::Mount));
        }
        let active = crate::mounts::active_mount_points(state).await.unwrap_or_default();
        if crate::mounts::is_mounted(&active, &m.mount_point) {
            return Ok(Some(ServiceKind::Mount));
        }
    }

    let jobs = crate::jobs::load_from_dir(config_dir).unwrap_or_default();
    if jobs.iter().any(|j| {
        j.auto_interval_minutes.is_some()
            && (remote_name_of(&j.source) == Some(remote_name) || remote_name_of(&j.destination) == Some(remote_name))
    }) {
        return Ok(Some(ServiceKind::Backup));
    }

    let bisync_jobs = crate::bisync::load_from_dir(config_dir).unwrap_or_default();
    if bisync_jobs.iter().any(|j| {
        j.auto_interval_minutes.is_some()
            && (remote_name_of(&j.path1) == Some(remote_name) || remote_name_of(&j.path2) == Some(remote_name))
    }) {
        return Ok(Some(ServiceKind::Bisync));
    }

    Ok(None)
}

/// Rifiuta se un servizio diverso da `activating` è già attivo per questo
/// remote — al massimo un servizio attivo alla volta. Va chiamata subito
/// prima di ogni azione che "attiva" un servizio: `mount_now`, e
/// creazione/modifica di un backup o bisync quando l'intervallo
/// automatico passa a `Some`.
pub(crate) async fn ensure_no_other_active_service(
    state: &RcdState,
    config_dir: &Path,
    remote_name: &str,
    activating: ServiceKind,
) -> Result<(), String> {
    match active_service_for_remote(state, config_dir, remote_name).await? {
        Some(active) if active != activating => {
            Err(format!("'{remote_name}' ha già {} attivo: disattivalo prima", active.label()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::{self, tests::TempDir};

    #[tokio::test]
    async fn active_service_for_remote_is_none_when_nothing_is_configured() {
        let dir = TempDir::new("activity-none");
        let state = rcd::build_state(dir.path.join("rclone.conf")).await;
        let result = active_service_for_remote(&state, &dir.path, "cubbit").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn active_service_for_remote_detects_a_mount_configured_with_auto_mount_even_if_not_mounted_yet() {
        let dir = TempDir::new("activity-auto-mount");
        let state = rcd::build_state(dir.path.join("rclone.conf")).await;
        crate::mounts::create_mount_in(&dir.path, "prova", "cubbit:x", "/tmp/prova-mount", true).unwrap();

        // Nessuna chiamata reale di mount qui: il mount NON è live-montato,
        // ma auto_mount=true basta perché conti come attivo (si
        // monterebbe da solo al prossimo avvio dell'app).
        let result = active_service_for_remote(&state, &dir.path, "cubbit").await.unwrap();
        assert_eq!(result, Some(ServiceKind::Mount));
    }

    #[tokio::test]
    async fn active_service_for_remote_ignores_a_mount_without_auto_mount_that_is_not_live_mounted() {
        let dir = TempDir::new("activity-manual-mount");
        let state = rcd::build_state(dir.path.join("rclone.conf")).await;
        crate::mounts::create_mount_in(&dir.path, "prova", "cubbit:x", "/tmp/prova-mount", false).unwrap();

        let result = active_service_for_remote(&state, &dir.path, "cubbit").await.unwrap();
        assert_eq!(result, None, "configurato ma senza auto_mount e non montato dal vivo non conta come attivo");
    }

    #[tokio::test]
    async fn active_service_for_remote_detects_an_auto_backup() {
        let dir = TempDir::new("activity-backup");
        let state = rcd::build_state(dir.path.join("rclone.conf")).await;
        crate::jobs::create_job_in(&dir.path, "prova", "/locale", "cubbit:x", Some(15), false).unwrap();

        let result = active_service_for_remote(&state, &dir.path, "cubbit").await.unwrap();
        assert_eq!(result, Some(ServiceKind::Backup));
    }

    #[tokio::test]
    async fn active_service_for_remote_ignores_a_backup_without_auto_interval() {
        let dir = TempDir::new("activity-backup-manual");
        let state = rcd::build_state(dir.path.join("rclone.conf")).await;
        crate::jobs::create_job_in(&dir.path, "prova", "/locale", "cubbit:x", None, false).unwrap();

        let result = active_service_for_remote(&state, &dir.path, "cubbit").await.unwrap();
        assert_eq!(result, None, "configurato ma solo manuale non conta come attivo");
    }

    #[tokio::test]
    async fn active_service_for_remote_detects_an_auto_bisync() {
        let dir = TempDir::new("activity-bisync");
        let state = rcd::build_state(dir.path.join("rclone.conf")).await;
        crate::bisync::create_bisync_job_in(&dir.path, "prova", "/locale", "cubbit:x", Some(15)).unwrap();

        let result = active_service_for_remote(&state, &dir.path, "cubbit").await.unwrap();
        assert_eq!(result, Some(ServiceKind::Bisync));
    }

    #[tokio::test]
    async fn ensure_no_other_active_service_allows_reactivating_the_same_kind() {
        let dir = TempDir::new("activity-same-kind");
        let state = rcd::build_state(dir.path.join("rclone.conf")).await;
        crate::jobs::create_job_in(&dir.path, "prova", "/locale", "cubbit:x", Some(15), false).unwrap();

        let result = ensure_no_other_active_service(&state, &dir.path, "cubbit", ServiceKind::Backup).await;
        assert!(result.is_ok(), "riattivare lo stesso tipo già attivo non deve essere bloccato: {result:?}");
    }

    #[tokio::test]
    async fn ensure_no_other_active_service_rejects_a_different_kind() {
        let dir = TempDir::new("activity-different-kind");
        let state = rcd::build_state(dir.path.join("rclone.conf")).await;
        crate::jobs::create_job_in(&dir.path, "prova", "/locale", "cubbit:x", Some(15), false).unwrap();

        let result = ensure_no_other_active_service(&state, &dir.path, "cubbit", ServiceKind::Bisync).await;
        assert!(result.is_err(), "il backup è già attivo: attivare bisync sullo stesso remote deve essere rifiutato");
    }
}
