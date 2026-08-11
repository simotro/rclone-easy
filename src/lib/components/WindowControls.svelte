<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import Icon from "./Icon.svelte";

  // Sostituisce il pulsante nativo della barra del titolo, rimossa del
  // tutto (`decorations: false` in tauri.conf.json) per un bug di tao su
  // Wayland/KDE che rendeva la X non cliccabile — vedi `tray.rs`. Nasconde
  // soltanto (stessa azione della X di sempre, `hide_window` in tray.rs):
  // niente secondo pulsante "chiudi definitivamente" accanto a questo, per
  // non confondere l'utente — l'uscita vera resta raggiungibile solo da
  // "Esci" nel menu della tray, unico posto dove ci si aspetta un'azione
  // distruttiva del genere.
  async function hide() {
    await invoke("hide_window");
  }
</script>

<button
  type="button"
  class="window-control"
  onclick={hide}
  title="Nascondi la finestra — Rclone Easy resta attivo in background, trovi l'icona nella tray di sistema"
  aria-label="Nascondi la finestra"
>
  <Icon kind="quit" />
</button>

<style>
.window-control {
  background: none;
  border: 1px solid var(--border-color);
  box-shadow: none;
  padding: 0.5em;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
}

.window-control:hover {
  color: var(--text-color);
}
</style>
