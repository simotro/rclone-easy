<div align="center">
  <img src="docs/screenshot-dark.png" alt="Rclone Easy — schermata principale" width="700">
</div>

# Rclone Easy

**Un'interfaccia grafica semplice per [rclone](https://rclone.org/)** — pensata come alternativa più leggera a client come Insync, Google Drive Sync o MEGAsync, ma capace di collegarsi a decine di provider cloud diversi (S3 e compatibili, Backblaze B2, MEGA, Google Drive, Dropbox, OneDrive...) grazie al motore di rclone che lavora dietro le quinte.

Costruita con [Tauri 2](https://tauri.app/) (Rust) + [SvelteKit](https://kit.svelte.dev/) (Svelte 5).

> [!WARNING]
> **Progetto sperimentale, in sviluppo attivo.** Non è ancora stato sottoposto a un uso estensivo su larga scala e potrebbe contenere bug, anche seri. **Fai sempre un backup dei tuoi file prima di usare l'app** per mount, backup o sincronizzazioni — soprattutto la prima volta che la usi con dati importanti. Usala a tuo rischio.

## Cosa fa

- **Mount**: collega un remote cloud come se fosse una cartella locale.
- **Backup**: sincronizzazione in una sola direzione (locale → cloud o viceversa), con protezione di default contro cancellazioni accidentali sulla destinazione.
- **Sincronizzazione bidirezionale**: tiene allineate due cartelle (locale e remota) in entrambe le direzioni, con gestione esplicita dei conflitti.
- **Automazione**: ogni backup/sincronizzazione può girare da solo a intervalli regolari, senza bisogno di cron o systemd timer.
- **Icona nella tray**: stato a colpo d'occhio (in corso / ultimo errore), menu rapido per montare/smontare e avviare i job direttamente da lì.
- **Backup/ripristino cifrato** della configurazione dell'app (protetto da password: le credenziali dei tuoi remote non finiscono mai su disco in chiaro).
- **Wizard guidato** per collegare un provider, incluso un percorso passo-passo per creare un proprio client OAuth per Google Drive (Google ritirerà l'identità condivisa storica di rclone nel corso del 2026).

## Provare l'app

Il progetto è ancora agli inizi: non ci sono ancora build precompilate, va compilato da sorgente.

```bash
git clone https://github.com/simotro/rclone-easy.git
cd rclone-easy
npm install
./scripts/fetch-rclone-sidecar.sh   # scarica il binario rclone incluso nell'app (solo Linux x86_64 per ora)
npm run tauri dev
```

Richiede [Rust](https://www.rust-lang.org/tools/install) e le [dipendenze di sistema di Tauri](https://tauri.app/start/prerequisites/) già installate.

## Segnalazioni e idee

Se la provi e trovi un bug, o hai un'idea per migliorarla, [apri una issue](../../issues) — segnalazioni e proposte sono benvenute, il progetto ne ha bisogno proprio ora che è agli inizi.

## Licenza

[MIT](LICENSE)
