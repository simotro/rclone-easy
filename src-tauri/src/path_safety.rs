use std::path::{Path, PathBuf};

/// Cestino nativo del sistema operativo (specifica XDG Trash: `$topdir/
/// .Trash-$uid`) che un file manager crea DENTRO una cartella quando questa
/// viene vista come un filesystem a parte — capita tipicamente se quella
/// stessa cartella è stata usata come punto di montaggio FUSE (la funzione
/// "Mount" di questa app monta proprio così) prima di essere riconfigurata
/// per bisync/backup: la cartella resta lì, indistinguibile da un contenuto
/// vero, e bisync/backup la risincronizzerebbe come tale — compreso
/// ricaricare sul remote file che l'utente pensava di aver cancellato per
/// sempre. `*` al posto dello uid specifico: non ha senso legare
/// l'esclusione all'utente di QUESTA macchina, la cartella può essere stata
/// creata da chiunque abbia mai montato/navigato quel percorso.
pub(crate) const OS_TRASH_EXCLUDE: &str = "/.Trash-*/**";

/// Home directory dell'utente corrente, letta dalla variabile d'ambiente
/// giusta per piattaforma — niente crate esterna (`dirs`/`home`) solo per
/// questo, coerente con le dipendenze già minimali del progetto.
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let var = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let var = std::env::var_os("HOME");
    var.map(PathBuf::from)
}

/// `true` se `fs` è un percorso locale che coincide esattamente con la
/// radice del filesystem (`/` su Unix, una lettera di unità bare come `C:\`
/// su Windows) — mai un percorso remoto (`Path::is_absolute` lo esclude,
/// stessa logica già usata altrove per distinguerli) né una sottocartella
/// della radice. `Path::parent()` è `None` solo per una radice vera: unico
/// modo corretto per riconoscerla indipendentemente dalla piattaforma, a
/// differenza di un confronto testuale con `"/"` che ignorerebbe Windows.
pub(crate) fn is_filesystem_root(fs: &str) -> bool {
    let path = Path::new(fs);
    path.is_absolute() && path.parent().is_none()
}

/// `true` se `fs` è un percorso locale che coincide esattamente con la home
/// directory dell'utente corrente — non una sua sottocartella. A differenza
/// della radice del filesystem, sincronizzare l'intera home è un pattern di
/// backup legittimo (tipo Time Machine): questo controllo non blocca nulla
/// da solo, serve solo a far scattare un avviso di conferma lato frontend.
pub(crate) fn is_home_directory(fs: &str) -> bool {
    let path = Path::new(fs);
    path.is_absolute() && home_dir().is_some_and(|home| path == home)
}

/// Usata dal frontend subito dopo che l'utente sceglie una cartella locale
/// (backup/bisync), prima ancora di salvare il job — così l'avviso compare
/// nel momento in cui l'utente fa la scelta, non solo al salvataggio.
/// `"root"` per la radice del filesystem (bloccata sempre, senza eccezioni,
/// lato backend in `jobs::create_job_in`/`bisync::create_bisync_job_in` e
/// affini), `"home"` per la home directory intera (non bloccata: solo un
/// avviso da confermare), `None` per qualunque altro percorso.
#[tauri::command]
pub fn check_dangerous_path(path: String) -> Option<&'static str> {
    if is_filesystem_root(&path) {
        Some("root")
    } else if is_home_directory(&path) {
        Some("home")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_filesystem_root_recognizes_the_unix_root() {
        assert!(is_filesystem_root("/"));
    }

    #[test]
    fn is_filesystem_root_rejects_a_subfolder_of_root() {
        assert!(!is_filesystem_root("/home"));
    }

    #[test]
    fn is_filesystem_root_rejects_a_remote_path() {
        assert!(!is_filesystem_root("cubbit:"));
        assert!(!is_filesystem_root("cubbit:/"));
    }

    #[test]
    fn is_home_directory_recognizes_the_current_home() {
        let home = home_dir().expect("il test presuppone HOME/USERPROFILE impostata");
        assert!(is_home_directory(home.to_str().unwrap()));
    }

    #[test]
    fn is_home_directory_rejects_a_subfolder_of_home() {
        let home = home_dir().expect("il test presuppone HOME/USERPROFILE impostata");
        let subfolder = home.join("Documenti");
        assert!(!is_home_directory(subfolder.to_str().unwrap()));
    }

    #[test]
    fn is_home_directory_rejects_a_remote_path() {
        assert!(!is_home_directory("cubbit:qualcosa"));
    }
}
