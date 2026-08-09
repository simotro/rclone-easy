<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { goto } from "$app/navigation";
  import { page } from "$app/stores";

  type RemoteForEdit = { kind: string; parameters: Record<string, string> };
  type S3ProviderOption = { value: string; help: string };
  type S3RegionOption = { value: string; help: string; providers: string[] };
  type LoadState = { status: "loading" } | { status: "ok" } | { status: "error"; message: string };

  const remoteName = $derived(decodeURIComponent($page.params.name ?? ""));

  let kind = $state("");
  let loadState = $state<LoadState>({ status: "loading" });
  let submitting = $state(false);
  let errorMessage = $state<string | null>(null);

  // S3 e compatibili
  let s3Provider = $state("Other");
  let s3Endpoint = $state("");
  let s3AccessKeyId = $state("");
  let s3SecretAccessKey = $state("");
  let s3Region = $state("");
  let s3Providers = $state<S3ProviderOption[]>([]);
  let s3Regions = $state<S3RegionOption[]>([]);
  // Solo un cambio di provider fatto dall'utente deve poter azzerare la
  // regione già salvata — al caricamento iniziale la regione esistente va
  // sempre rispettata anche se non compare tra gli esempi noti di rclone
  // (endpoint personalizzati possono avere regioni non catalogate).
  let s3ProviderTouchedByUser = $state(false);

  // Backblaze B2
  let b2Account = $state("");
  let b2Key = $state("");

  // MEGA
  let megaUser = $state("");
  let megaPass = $state("");

  // Google Drive: nessun campo modificabile qui (vedi il banner sotto), solo
  // se ha già un client_id proprio o usa ancora l'identità condivisa che
  // Google ritirerà nel corso del 2026 — `client_id` è l'unico campo che
  // `get_remote_for_edit` espone per "drive", vedi remotes.rs.
  let driveHasOwnClientId = $state(false);

  $effect(() => {
    invoke<RemoteForEdit>("get_remote_for_edit", { name: remoteName })
      .then((result) => {
        kind = result.kind;
        if (kind === "s3") {
          s3Provider = result.parameters.provider ?? "Other";
          s3Endpoint = result.parameters.endpoint ?? "";
          s3AccessKeyId = result.parameters.access_key_id ?? "";
          s3Region = result.parameters.region ?? "";
          invoke<S3ProviderOption[]>("list_s3_providers")
            .then((providers) => (s3Providers = providers))
            .catch(() => {});
          invoke<S3RegionOption[]>("list_s3_regions")
            .then((regions) => (s3Regions = regions))
            .catch(() => {});
        } else if (kind === "b2") {
          b2Account = result.parameters.account ?? "";
        } else if (kind === "mega") {
          megaUser = result.parameters.user ?? "";
        } else if (kind === "drive") {
          driveHasOwnClientId = (result.parameters.client_id ?? "").trim() !== "";
        }
        loadState = { status: "ok" };
      })
      .catch((error) => (loadState = { status: "error", message: String(error) }));
  });

  let s3RegionsForProvider = $derived(s3Regions.filter((r) => r.providers.includes(s3Provider)));

  $effect(() => {
    if (
      s3ProviderTouchedByUser &&
      s3RegionsForProvider.length > 0 &&
      !s3RegionsForProvider.some((r) => r.value === s3Region)
    ) {
      s3Region = "";
    }
  });

  function buildParameters(): Record<string, string> {
    const params: Record<string, string> = {};
    const add = (key: string, value: string) => {
      if (value.trim() !== "") params[key] = value.trim();
    };
    if (kind === "s3") {
      add("provider", s3Provider);
      add("endpoint", s3Endpoint);
      add("access_key_id", s3AccessKeyId);
      add("secret_access_key", s3SecretAccessKey);
      add("region", s3Region);
    } else if (kind === "b2") {
      add("account", b2Account);
      add("key", b2Key);
    } else if (kind === "mega") {
      add("user", megaUser);
      add("pass", megaPass);
    }
    return params;
  }

  async function submit() {
    submitting = true;
    errorMessage = null;
    try {
      await invoke("update_remote", { name: remoteName, parameters: buildParameters() });
      goto("/");
    } catch (error) {
      errorMessage = String(error);
    } finally {
      submitting = false;
    }
  }
</script>

<main class="container">
  <a href="/" class="back-link">← Torna alla schermata principale</a>
  <h1>Modifica {remoteName}</h1>

  {#if loadState.status === "loading"}
    <p>Caricamento…</p>
  {:else if loadState.status === "error"}
    <p class="error">✗ {loadState.message}</p>
  {:else if kind === "drive" && !driveHasOwnClientId}
    <p class="warning">
      Questo remote Google Drive usa ancora l'identità condivisa di rclone, che <strong>Google disattiverà nel corso
      del 2026</strong> — da quel momento smetterà di funzionare. Per continuare a usarlo serve un client Google
      tuo, gratuito e veloce da creare. I remote autorizzati via browser non sono ancora modificabili da qui: per
      passare a un client tuo, elimina questo remote dalla schermata principale e ricrealo con lo stesso nome
      («{remoteName}») da "Aggiungi remote" → Google Drive, questa volta compilando "Usa un client Google tuo".
      Mount, backup e sincronizzazioni collegati a questo nome restano validi, non serve ricrearli.
    </p>
  {:else if kind === "drive"}
    <p class="ok">✓ Questo remote Google Drive usa già un client Google tuo, non è coinvolto dal ritiro
      dell'identità condivisa di rclone previsto da Google nel corso del 2026.</p>
  {:else if kind === "dropbox" || kind === "onedrive"}
    <p class="warning">
      I remote autorizzati via browser (Dropbox, OneDrive) non sono ancora modificabili da qui. Se devi cambiare
      account, elimina questo remote dalla pagina precedente e ricrealo.
    </p>
  {:else}
    <form onsubmit={(e) => { e.preventDefault(); submit(); }}>
      {#if kind === "s3"}
        <label>
          Provider
          <select bind:value={s3Provider} onchange={() => (s3ProviderTouchedByUser = true)}>
            <option value="Other">Altro / endpoint personalizzato</option>
            {#each s3Providers as p (p.value)}
              <option value={p.value} title={p.help}>{p.value}</option>
            {/each}
          </select>
        </label>
        <label>
          Endpoint
          <input type="text" bind:value={s3Endpoint} placeholder="es. s3.cubbit.eu, s3.wasabisys.com" />
        </label>
        <label>
          Access Key ID
          <input type="text" bind:value={s3AccessKeyId} />
        </label>
        <label>
          Secret Access Key
          <input type="password" bind:value={s3SecretAccessKey} placeholder="lascia vuoto per non cambiarla" />
        </label>
        <label>
          Regione (facoltativa)
          {#if s3RegionsForProvider.length > 0}
            <select bind:value={s3Region}>
              <option value="">Nessuna / non specificata</option>
              {#each s3RegionsForProvider as r (r.value)}
                <option value={r.value} title={r.help}>{r.value || "(vuota)"} — {r.help}</option>
              {/each}
            </select>
          {:else}
            <input type="text" bind:value={s3Region} placeholder="es. eu-west-1" />
          {/if}
        </label>
      {:else if kind === "b2"}
        <label>
          Account ID
          <input type="text" bind:value={b2Account} />
        </label>
        <label>
          Application Key
          <input type="password" bind:value={b2Key} placeholder="lascia vuoto per non cambiarla" />
        </label>
      {:else if kind === "mega"}
        <label>
          Email
          <input type="text" bind:value={megaUser} />
        </label>
        <label>
          Password
          <input type="password" bind:value={megaPass} placeholder="lascia vuoto per non cambiarla" />
        </label>
      {/if}

      {#if errorMessage}
        <p class="error">✗ Connessione non riuscita: {errorMessage}</p>
      {/if}

      <div class="actions">
        <button type="submit" disabled={submitting}>
          {submitting ? "Verifica in corso…" : "Verifica e salva"}
        </button>
      </div>
    </form>
  {/if}
</main>

<style>
.container {
  margin: 0 auto;
  max-width: 40em;
  padding: 4vh 1.5em;
}

.back-link {
  display: inline-block;
  color: var(--accent);
}

.warning {
  margin-top: 1.5em;
  padding: 0.8em 1em;
  border-radius: 8px;
  background-color: var(--warning-bg);
  color: var(--warning-text);
  font-size: 0.9em;
}

form {
  display: flex;
  flex-direction: column;
  gap: 1em;
  margin-top: 1.5em;
}

label {
  display: flex;
  flex-direction: column;
  gap: 0.3em;
  font-weight: 500;
}

.actions {
  display: flex;
  justify-content: flex-end;
}
</style>
