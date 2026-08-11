<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { goto } from "$app/navigation";
  import { t } from "$lib/i18n";

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
  <h1>{$t("newRemote.title")}</h1>

  {#if step === 1}
    <a href="/" class="back-link">← {$t("newRemote.backToHome")}</a>
    <p class="subtitle">{$t("newRemote.subtitle")}</p>
    <div class="cards">
      <button class="card" onclick={() => goto("/importa-remote")}>
        <strong>{$t("home.importExistingRemotes")}</strong>
        <span>{$t("newRemote.importCardDesc")}</span>
      </button>
      <button class="card" onclick={() => selectKind("s3")}>
        <strong>{$t("newRemote.s3Card")}</strong>
        <span>{$t("newRemote.s3CardDesc")}</span>
      </button>
      <button class="card" onclick={() => selectKind("b2")}>
        <strong>Backblaze B2</strong>
        <span>{$t("newRemote.b2CardDesc")}</span>
      </button>
      <button class="card" onclick={() => selectKind("mega")}>
        <strong>MEGA</strong>
        <span>{$t("newRemote.megaCardDesc")}</span>
      </button>
      <button class="card" onclick={() => selectKind("drive")}>
        <strong>Google Drive</strong>
        <span>{$t("newRemote.driveCardDesc")}</span>
      </button>
      <button class="card" onclick={() => selectKind("dropbox")}>
        <strong>Dropbox</strong>
        <span>{$t("newRemote.oauthCardDesc")}</span>
      </button>
      <button class="card" onclick={() => selectKind("onedrive")}>
        <strong>OneDrive</strong>
        <span>{$t("newRemote.oauthCardDesc")}</span>
      </button>
    </div>
  {:else if step === 2 && isOAuthKind(kind)}
    <form onsubmit={(e) => { e.preventDefault(); submitOAuth(); }}>
      <label>
        {$t("newRemote.remoteNameLabel")}
        <input type="text" bind:value={name} placeholder={$t("newRemote.remoteNamePlaceholder")} disabled={oauthWaiting} />
      </label>

      {#if kind === "drive" && !oauthWaiting}
        <div class="own-client-box">
          <p class="own-client-title">{$t("newRemote.ownClientTitle")}</p>
          <p class="own-client-explainer">
            {$t("newRemote.ownClientExplainer")}
          </p>
          <details class="own-client-steps">
            <summary>{$t("newRemote.ownClientHow")}</summary>
            <ol>
              <li>
                {$t("newRemote.step1Before")}
                <button type="button" class="link-button inline" onclick={openGoogleCloudConsole}>
                  console.cloud.google.com
                </button>
                .
              </li>
              <li>
                {$t("newRemote.step2")}
              </li>
              <li>
                {$t("newRemote.step3Before")}
                <strong>Google Drive API</strong> {$t("newRemote.step3After")}
              </li>
              <li>
                {$t("newRemote.step4Before")}
                <strong>{$t("newRemote.externalType")}</strong> {$t("newRemote.step4After")}
              </li>
              <li>
                {$t("newRemote.step5Before")} <code>drive</code>,
                <code>drive.metadata.readonly</code> {$t("newRemote.step5And")} <code>docs</code> {$t("newRemote.step5After")}
              </li>
              <li>
                {$t("newRemote.step6")}
              </li>
              <li>
                {$t("newRemote.step7Before")} <strong>{$t("newRemote.desktopAppType")}</strong>.
              </li>
              <li>{$t("newRemote.step8")}</li>
            </ol>
            <button type="button" class="link-button" onclick={openGoogleDriveClientIdGuide}>
              {$t("newRemote.officialGuideLink")}
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
            <p class="error">✗ {$t("newRemote.clientIdSecretBothRequired")}</p>
          {/if}
        </div>
      {/if}

      {#if oauthWaiting && !oauthQuestion}
        <div class="oauth-wait">
          <p>{$t("newRemote.oauthOpenedBrowser", { values: { provider: OAUTH_LABELS[kind ?? ""] } })}</p>
          {#if oauthUrl}
            <button type="button" class="link-button" onclick={openOAuthUrlManually}>
              {$t("newRemote.oauthOpenLinkManually")}
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
              <button type="button" onclick={() => answerOAuthQuestion(oauthFreeTextAnswer)}>{$t("common.confirm")}</button>
            </div>
          {/if}
        </div>
      {/if}

      {#if errorMessage}
        <p class="error">✗ {errorMessage}</p>
      {/if}

      <div class="actions">
        {#if oauthWaiting}
          <button type="button" onclick={cancelOAuth}>{$t("common.cancel")}</button>
        {:else}
          <button type="button" onclick={backToStep1} disabled={submitting}>{$t("newRemote.back")}</button>
          <button type="submit" disabled={!canSubmit || submitting}>
            {$t("newRemote.authorizeWith", { values: { provider: OAUTH_LABELS[kind ?? ""] } })}
          </button>
        {/if}
      </div>
    </form>
  {:else if step === 2}
    <form onsubmit={(e) => { e.preventDefault(); submit(); }}>
      <label>
        {$t("newRemote.remoteNameLabel")}
        <input type="text" bind:value={name} placeholder={$t("newRemote.remoteNamePlaceholder")} />
      </label>

      {#if kind === "s3"}
        <label>
          Provider
          <select bind:value={s3Provider}>
            <option value="Other">{$t("newRemote.otherCustomEndpoint")}</option>
            {#each s3Providers as p (p.value)}
              <option value={p.value} title={p.help}>{p.value}</option>
            {/each}
          </select>
        </label>
        <label>
          {$t("newRemote.endpointLabel")}
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
          {$t("newRemote.regionLabel")}
          {#if s3RegionsForProvider.length > 0}
            <select bind:value={s3Region}>
              <option value="">{$t("newRemote.noRegionSpecified")}</option>
              {#each s3RegionsForProvider as r (r.value)}
                <option value={r.value} title={r.help}>{r.value || $t("newRemote.emptyRegion")} — {r.help}</option>
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
          {$t("newRemote.emailLabel")}
          <input type="text" bind:value={megaUser} />
        </label>
        <label>
          {$t("unlock.passwordLabel")}
          <input type="password" bind:value={megaPass} />
        </label>
        <label>
          {$t("newRemote.twoFaLabel")}
          <input type="text" bind:value={megaTwoFa} />
        </label>
      {/if}

      {#if errorMessage}
        <p class="error">✗ {$t("newRemote.connectionFailed", { values: { error: errorMessage } })}</p>
      {/if}

      <div class="actions">
        <button type="button" onclick={backToStep1} disabled={submitting}>{$t("newRemote.back")}</button>
        <button type="submit" disabled={!canSubmit || submitting}>
          {submitting ? $t("importPanel.checking") : $t("newRemote.verifyAndSave")}
        </button>
      </div>
    </form>
  {:else if step === 3}
    <p class="ok">✓ {$t("newRemote.remoteCreated", { values: { name } })}</p>
    <a href="/">{$t("newRemote.backToHome")}</a>
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
