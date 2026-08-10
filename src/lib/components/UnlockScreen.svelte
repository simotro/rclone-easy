<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import PasswordField from "./PasswordField.svelte";

  let { onUnlocked }: { onUnlocked: () => void } = $props();

  let password = $state("");
  let submitting = $state(false);
  let error = $state<string | null>(null);

  async function submit() {
    if (password.trim() === "" || submitting) return;
    submitting = true;
    error = null;
    try {
      await invoke("unlock_config", { password });
      onUnlocked();
    } catch (e) {
      // Password sbagliata o config corrotta: si resta qui, l'utente può
      // ritentare senza dover riavviare l'app (vedi rcd::unlock).
      error = String(e);
    } finally {
      submitting = false;
    }
  }
</script>

<main class="container">
  <div class="unlock-box">
    <img src="/app-icon.png" alt="" class="app-icon" />
    <h1>Rclone Easy</h1>
    <p class="subtitle">La configurazione è protetta da una password. Inseriscila per continuare.</p>
    <form onsubmit={(e) => { e.preventDefault(); submit(); }}>
      <PasswordField bind:value={password} label="Password" autofocus disabled={submitting} />
      {#if error}
        <p class="error">✗ {error}</p>
      {/if}
      <button type="submit" disabled={submitting || password.trim() === ""}>
        {submitting ? "Sblocco in corso…" : "Sblocca"}
      </button>
    </form>
  </div>
</main>

<style>
.container {
  display: flex;
  align-items: center;
  justify-content: center;
  /* 100% e non 100vh: questo elemento vive dentro `.app-body` di
     +layout.svelte, che occupa già lo spazio rimasto sotto la striscia
     trascinabile in alto — 100vh (l'intera finestra) trabocca di quella
     striscia, causando uno scroll indesiderato. */
  min-height: 100%;
  padding: 1.5em;
}

.unlock-box {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.6em;
  width: min(24em, 100%);
  text-align: center;
}

.app-icon {
  width: 3.5em;
  height: 3.5em;
}

h1 {
  margin: 0;
}

.subtitle {
  color: var(--text-muted);
  margin: 0 0 1em;
}

form {
  display: flex;
  flex-direction: column;
  gap: 1em;
  width: 100%;
  text-align: left;
}

form button[type="submit"] {
  align-self: center;
}
</style>
