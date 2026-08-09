<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { goto } from "$app/navigation";

  type ProviderKind = "s3" | "b2" | "mega" | "drive" | "dropbox" | "onedrive";
  type S3ProviderOption = { value: string; help: string };
  type S3RegionOption = { value: string; help: string; providers: string[] };

  const OAUTH_LABELS: Record<string, string> = { drive: "Google Drive", dropbox: "Dropbox", onedrive: "OneDrive" };
  function isOAuthKind(k: ProviderKind | null): boolean {
    return k === "drive" || k === "dropbox" || k === "onedrive";
  }

  let step = $state<1 | 2 | 3>(1);
  let kind = $state<ProviderKind | null>(null);

  let name = $state("");

  // S3 e compatibili
  let s3Provider = $state("Other");
  let s3Endpoint = $state("");
  let s3AccessKeyId = $state("");
  let s3SecretAccessKey = $state("");
  let s3Region = $state("");
  let s3Providers = $state<S3ProviderOption[]>([]);
  let s3Regions = $state<S3RegionOption[]>([]);

  // Backblaze B2
  let b2Account = $state("");
  let b2Key = $state("");

  // MEGA
  let megaUser = $state("");
  let megaPass = $state("");
  let megaTwoFa = $state("");

  // Google Drive — client OAuth proprio dell'utente, non condiviso: Google
  // ritirerà l'identità condivisa usata finora da rclone nel corso del
  // 2026 (vedi il modulo "Usa un client Google tuo" più sotto e
  // `oauth_remote.rs::initial_parameters`). Facoltativo per ora (l'identità
  // condivisa funziona ancora), ma consigliato esplicitamente.
  let driveClientId = $state("");
  let driveClientSecret = $state("");

  let submitting = $state(false);
  let errorMessage = $state<string | null>(null);
  let oauthUrl = $state<string | null>(null);
  let oauthWaiting = $state(false);

  type OAuthQuestion = { help: string; examples: { value: string; help: string }[]; defaultValue: string };
  let oauthQuestion = $state<OAuthQuestion | null>(null);
  let oauthFreeTextAnswer = $state("");

  function selectKind(k: ProviderKind) {
    kind = k;
    step = 2;
    errorMessage = null;
  }

  function backToStep1() {
    step = 1;
    errorMessage = null;
  }

  $effect(() => {
    if (kind === "s3" && s3Providers.length === 0) {
      invoke<S3ProviderOption[]>("list_s3_providers")
        .then((providers) => (s3Providers = providers))
        .catch(() => {
          // L'elenco provider è solo un aiuto per compilare il campo: se non
          // arriva l'utente può comunque scrivere il nome del provider a mano.
        });
      invoke<S3RegionOption[]>("list_s3_regions")
        .then((regions) => (s3Regions = regions))
        .catch(() => {});
    }
  });

  // Solo le regioni pertinenti al provider scelto (rclone associa ogni
  // esempio di regione a uno o più provider) — "Other" non ha regioni
  // note in anticipo, resta un campo libero in quel caso.
  let s3RegionsForProvider = $derived(s3Regions.filter((r) => r.providers.includes(s3Provider)));

  $effect(() => {
    // Se si cambia provider e la regione già scelta non è più tra quelle
    // proposte, non ha più senso lasciarla: eviterebbe di salvare un
    // remote con un accoppiamento provider/regione inconsistente.
    if (s3RegionsForProvider.length > 0 && !s3RegionsForProvider.some((r) => r.value === s3Region)) {
      s3Region = "";
    }
  });

  let canSubmit = $derived.by(() => {
    if (name.trim() === "") return false;
    if (kind === "s3") return s3AccessKeyId.trim() !== "" && s3SecretAccessKey.trim() !== "";
    if (kind === "b2") return b2Account.trim() !== "" && b2Key.trim() !== "";
    if (kind === "mega") return megaUser.trim() !== "" && megaPass.trim() !== "";
    if (kind === "drive") return (driveClientId.trim() === "") === (driveClientSecret.trim() === "");
    if (isOAuthKind(kind)) return true;
    return false;
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
      add("2fa", megaTwoFa);
    }
    return params;
  }

  async function submit() {
    if (!kind || !canSubmit) return;
    submitting = true;
    errorMessage = null;
    try {
      await invoke("create_remote", { name: name.trim(), kind, parameters: buildParameters() });
      step = 3;
    } catch (error) {
      errorMessage = String(error);
    } finally {
      submitting = false;
    }
  }

  async function submitOAuth() {
    if (!kind || !canSubmit) return;
    submitting = true;
    errorMessage = null;
    oauthUrl = null;
    oauthQuestion = null;
    oauthWaiting = true;

    const unlistenUrl = await listen<{ url: string }>("rclone-easy://oauth-url", (event) => {
      oauthUrl = event.payload.url;
    });
    const unlistenQuestion = await listen<OAuthQuestion>("rclone-easy://oauth-question", (event) => {
      oauthQuestion = event.payload;
      oauthFreeTextAnswer = event.payload.defaultValue;
    });

    try {
      // Le domande impreviste che alcuni provider pongono dopo
      // l'autorizzazione (es. tipo di account OneDrive) non hanno una
      // risposta precompilata: il backend mette in pausa il flusso e
      // arriva qui come evento rclone-easy://oauth-question, gestito sotto.
      await invoke("create_oauth_remote", {
        name: name.trim(),
        kind,
        extraAnswers: {},
        clientId: kind === "drive" && driveClientId.trim() !== "" ? driveClientId.trim() : null,
        clientSecret: kind === "drive" && driveClientSecret.trim() !== "" ? driveClientSecret.trim() : null,
      });
      step = 3;
    } catch (error) {
      errorMessage = String(error);
    } finally {
      submitting = false;
      oauthWaiting = false;
      oauthQuestion = null;
      unlistenUrl();
      unlistenQuestion();
    }
  }

  async function answerOAuthQuestion(answer: string) {
    oauthQuestion = null;
    await invoke("answer_oauth_question", { answer });
  }

  async function cancelOAuth() {
    await invoke("cancel_oauth");
  }

  async function openOAuthUrlManually() {
    if (oauthUrl) await openUrl(oauthUrl);
  }

  async function openGoogleDriveClientIdGuide() {
    await openUrl("https://rclone.org/drive/#making-your-own-client-id");
  }

  async function openGoogleCloudConsole() {
    await openUrl("https://console.cloud.google.com/");
  }
</script>

<main class="container">
  <h1>Aggiungi un remote</h1>

  {#if step === 1}
    <a href="/" class="back-link">← Torna alla schermata principale</a>
    <p class="subtitle">Scegli il tipo di spazio di archiviazione da collegare.</p>
    <div class="cards">
      <button class="card" onclick={() => goto("/importa-remote")}>
        <strong>Importa remote esistenti</strong>
        <span>Hai già dei remote configurati con rclone? Importali invece di ricrearli.</span>
      </button>
      <button class="card" onclick={() => selectKind("s3")}>
        <strong>S3 e compatibili</strong>
        <span>Incluso Cubbit, Wasabi e altri provider S3-compatibili con endpoint personalizzato.</span>
      </button>
      <button class="card" onclick={() => selectKind("b2")}>
        <strong>Backblaze B2</strong>
        <span>Account ID e Application Key.</span>
      </button>
      <button class="card" onclick={() => selectKind("mega")}>
        <strong>MEGA</strong>
        <span>Email e password dell'account.</span>
      </button>
      <button class="card" onclick={() => selectKind("drive")}>
        <strong>Google Drive</strong>
        <span>Autorizzazione nel browser. Google ritirerà l'identità condivisa usata finora da rclone durante il 2026 — al passo successivo puoi già creare la tua.</span>
      </button>
      <button class="card" onclick={() => selectKind("dropbox")}>
        <strong>Dropbox</strong>
        <span>Autorizzazione nel browser.</span>
      </button>
      <button class="card" onclick={() => selectKind("onedrive")}>
        <strong>OneDrive</strong>
        <span>Autorizzazione nel browser.</span>
      </button>
    </div>
  {:else if step === 2 && isOAuthKind(kind)}
    <form onsubmit={(e) => { e.preventDefault(); submitOAuth(); }}>
      <label>
        Nome del remote
        <input type="text" bind:value={name} placeholder="es. lavoro-cloud" disabled={oauthWaiting} />
      </label>

      {#if kind === "drive" && !oauthWaiting}
        <div class="own-client-box">
          <p class="own-client-title">Usa un client Google tuo (consigliato)</p>
          <p class="own-client-explainer">
            Google disattiverà l'identità condivisa usata finora da rclone nel corso del 2026: da quel momento
            Google Drive smetterà di funzionare per chi non ha un client proprio. Crearne uno richiede circa 5
            minuti su Google Cloud Console ed è gratuito. Puoi anche saltare questo passo per ora — l'identità
            condivisa funziona ancora — e farlo più avanti ricreando il remote.
          </p>
          <details class="own-client-steps">
            <summary>Come si crea?</summary>
            <ol>
              <li>
                Vai su
                <button type="button" class="link-button inline" onclick={openGoogleCloudConsole}>
                  console.cloud.google.com
                </button>
                .
              </li>
              <li>
                Clicca in alto a sinistra su "Seleziona un progetto": scegline uno esistente oppure clicca "Nuovo
                progetto" e dagli un nome, es. "Rclone".
              </li>
              <li>
                "API e servizi" → "Libreria": usa il filtro di ricerca in alto per trovare
                <strong>Google Drive API</strong> e abilitala.
              </li>
              <li>
                "API e servizi" → "Credenziali" → "Configura schermata di consenso" (se non l'hai già fatto): tipo
                utente <strong>Esterno</strong> (Interno solo se hai un account Google Workspace), nome app ed
                email di supporto.
              </li>
              <li>
                Nella sezione "Accesso ai dati": aggiungi gli ambiti <code>drive</code>,
                <code>drive.metadata.readonly</code> e <code>docs</code> (quelli che rclone stesso usa).
              </li>
              <li>
                Solo se hai scelto Esterno: nel menu "Pubblico" aggiungi il tuo indirizzo email tra gli "utenti di
                test" (l'app resta "non verificata": va bene per uso personale, ma solo gli utenti di test
                elencati possono accedere), poi pubblicala da lì.
              </li>
              <li>
                Nel menu "Client": clicca "Crea client", tipo <strong>Applicazione Desktop</strong>.
              </li>
              <li>Copia il Client ID e il Client Secret generati e incollali qui sotto.</li>
            </ol>
            <button type="button" class="link-button" onclick={openGoogleDriveClientIdGuide}>
              Guida ufficiale di rclone, per approfondire
            </button>
          </details>
          <label>
            Client ID
            <input type="text" bind:value={driveClientId} placeholder="xxxxxxxxxx.apps.googleusercontent.com" />
          </label>
          <label>
            Client Secret
            <input type="password" bind:value={driveClientSecret} />
          </label>
          {#if (driveClientId.trim() === "") !== (driveClientSecret.trim() === "")}
            <p class="error">✗ Servono sia il Client ID sia il Client Secret, non uno solo.</p>
          {/if}
        </div>
      {/if}

      {#if oauthWaiting && !oauthQuestion}
        <div class="oauth-wait">
          <p>Si è aperto il browser per l'autorizzazione con {OAUTH_LABELS[kind ?? ""]} — completa l'accesso lì.</p>
          {#if oauthUrl}
            <button type="button" class="link-button" onclick={openOAuthUrlManually}>
              Non si è aperto automaticamente? Apri il link
            </button>
          {/if}
        </div>
      {/if}

      {#if oauthQuestion}
        <div class="oauth-wait">
          <p>{oauthQuestion.help}</p>
          {#if oauthQuestion.examples.length > 0}
            <div class="oauth-question-choices">
              {#each oauthQuestion.examples as example (example.value)}
                <button type="button" onclick={() => answerOAuthQuestion(example.value)}>
                  {example.help}
                </button>
              {/each}
            </div>
          {:else}
            <div class="oauth-question-choices">
              <input type="text" bind:value={oauthFreeTextAnswer} />
              <button type="button" onclick={() => answerOAuthQuestion(oauthFreeTextAnswer)}>Conferma</button>
            </div>
          {/if}
        </div>
      {/if}

      {#if errorMessage}
        <p class="error">✗ {errorMessage}</p>
      {/if}

      <div class="actions">
        {#if oauthWaiting}
          <button type="button" onclick={cancelOAuth}>Annulla</button>
        {:else}
          <button type="button" onclick={backToStep1} disabled={submitting}>Indietro</button>
          <button type="submit" disabled={!canSubmit || submitting}>
            Autorizza con {OAUTH_LABELS[kind ?? ""]}
          </button>
        {/if}
      </div>
    </form>
  {:else if step === 2}
    <form onsubmit={(e) => { e.preventDefault(); submit(); }}>
      <label>
        Nome del remote
        <input type="text" bind:value={name} placeholder="es. lavoro-cloud" />
      </label>

      {#if kind === "s3"}
        <label>
          Provider
          <select bind:value={s3Provider}>
            <option value="Other">Altro / endpoint personalizzato</option>
            {#each s3Providers as p (p.value)}
              <option value={p.value} title={p.help}>{p.value}</option>
            {/each}
          </select>
        </label>
        <label>
          Endpoint (richiesto per quasi tutti i provider tranne AWS)
          <input type="text" bind:value={s3Endpoint} placeholder="es. s3.cubbit.eu, s3.wasabisys.com" />
        </label>
        <label>
          Access Key ID
          <input type="text" bind:value={s3AccessKeyId} />
        </label>
        <label>
          Secret Access Key
          <input type="password" bind:value={s3SecretAccessKey} />
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
          <input type="password" bind:value={b2Key} />
        </label>
      {:else if kind === "mega"}
        <label>
          Email
          <input type="text" bind:value={megaUser} />
        </label>
        <label>
          Password
          <input type="password" bind:value={megaPass} />
        </label>
        <label>
          Codice 2FA (facoltativo)
          <input type="text" bind:value={megaTwoFa} />
        </label>
      {/if}

      {#if errorMessage}
        <p class="error">✗ Connessione non riuscita: {errorMessage}</p>
      {/if}

      <div class="actions">
        <button type="button" onclick={backToStep1} disabled={submitting}>Indietro</button>
        <button type="submit" disabled={!canSubmit || submitting}>
          {submitting ? "Verifica in corso…" : "Verifica e salva"}
        </button>
      </div>
    </form>
  {:else if step === 3}
    <p class="ok">✓ Remote "{name}" creato e verificato correttamente.</p>
    <a href="/">Torna alla schermata principale</a>
  {/if}
</main>

<style>
.container {
  margin: 0 auto;
  max-width: 40em;
  padding: 4vh 1.5em;
}

.subtitle {
  color: var(--text-muted);
  margin-top: 1em;
}

.back-link {
  display: inline-block;
  color: var(--accent);
}

.cards {
  display: grid;
  gap: 0.75em;
  margin-top: 1.5em;
}

.card {
  display: flex;
  flex-direction: column;
  gap: 0.25em;
  text-align: left;
  padding: 0.9em 1.1em;
  border-radius: 8px;
  border: 1px solid transparent;
  background-color: var(--bg-surface);
  box-shadow: var(--shadow-md);
  cursor: pointer;
  font-family: inherit;
  color: inherit;
}

.card span {
  color: var(--text-muted);
  font-size: 0.9em;
}

.own-client-box {
  display: flex;
  flex-direction: column;
  gap: 0.7em;
  padding: 0.9em 1.1em;
  border-radius: 8px;
  background-color: var(--accent-bg);
}

.own-client-title {
  margin: 0;
  font-weight: 600;
}

.own-client-explainer {
  margin: 0;
  color: var(--text-muted);
  font-size: 0.9em;
}

.own-client-steps summary {
  cursor: pointer;
  color: var(--accent);
  font-size: 0.9em;
}

.own-client-steps ol {
  margin: 0.6em 0 0;
  padding-left: 1.3em;
  display: flex;
  flex-direction: column;
  gap: 0.4em;
  font-size: 0.9em;
}

.own-client-steps code {
  font-family: "Cascadia Code", "Fira Code", monospace;
  background-color: var(--code-bg);
  padding: 0.1em 0.4em;
  border-radius: 4px;
}

.link-button.inline {
  display: inline;
  margin-top: 0;
  font-size: inherit;
}

.oauth-wait {
  padding: 0.9em 1.1em;
  border-radius: 8px;
  background-color: var(--accent-bg);
}

.oauth-wait p {
  margin: 0;
}

.oauth-question-choices {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5em;
  margin-top: 0.75em;
}

.oauth-question-choices input {
  flex: 1;
  min-width: 10em;
}

.link-button {
  display: inline-block;
  margin-top: 0.6em;
  background: none;
  border: none;
  box-shadow: none;
  padding: 0;
  color: var(--accent);
  cursor: pointer;
  font-size: 0.9em;
  text-align: left;
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
  justify-content: space-between;
  margin-top: 0.5em;
}
</style>
