<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { goto } from "$app/navigation";
  import { t } from "$lib/i18n";
  import ProviderIcon from "$lib/components/ProviderIcon.svelte";

  type ProviderKind = "s3" | "b2" | "mega" | "drive" | "dropbox" | "onedrive" | "nextcloud" | "owncloud" | "webdav" | "sftp";
  type S3ProviderOption = { value: string; help: string };
  type S3RegionOption = { value: string; help: string; providers: string[] };
  type S3EndpointOption = { value: string; help: string; providers: string[] };

  const OAUTH_LABELS: Record<string, string> = { drive: "Google Drive", dropbox: "Dropbox", onedrive: "OneDrive" };
  function isOAuthKind(k: ProviderKind | null): boolean {
    return k === "drive" || k === "dropbox" || k === "onedrive";
  }

  // Nextcloud/ownCloud/WebDAV sono tutti lo stesso backend rclone
  // ("webdav", distinto solo dal parametro "vendor") — tre card separate
  // qui solo perché è così che un utente li riconosce e li cerca in un
  // elenco, non perché servano tre tipi diversi lato rclone.
  function rcloneKindFor(k: ProviderKind): string {
    if (k === "nextcloud" || k === "owncloud" || k === "webdav") return "webdav";
    return k;
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
  // Non è un campo del backend s3 di rclone (il bucket fa sempre parte del
  // percorso quando si usa il remote, non della sua config) — serve solo a
  // verificare la connessione su un bucket specifico invece che sull'intero
  // account, per le chiavi S3 con permessi limitati a un solo bucket (vedi
  // il blocco "errorMessage" più sotto e verify_path lato backend).
  let s3Bucket = $state("");
  let s3Providers = $state<S3ProviderOption[]>([]);
  let s3Regions = $state<S3RegionOption[]>([]);
  let s3Endpoints = $state<S3EndpointOption[]>([]);

  // Backblaze B2
  let b2Account = $state("");
  let b2Key = $state("");

  // MEGA
  let megaUser = $state("");
  let megaPass = $state("");
  let megaTwoFa = $state("");

  // Nextcloud / ownCloud / WebDAV generico — stessi campi, riusati per
  // tutte e tre le card (vedi rcloneKindFor). `webdavHost` esiste solo per
  // nextcloud/owncloud: l'utente digita solo l'indirizzo del server (senza
  // https:// né percorso), `webdavUrl` — quello davvero inviato a rclone —
  // viene composto da soli (vedi l'effetto sotto), seguendo la convenzione
  // di percorso nota per quei due software. WebDAV generico non ha una
  // convenzione nota, resta un campo URL libero.
  let webdavHost = $state("");
  let webdavUrl = $state("");
  let webdavUser = $state("");
  let webdavPass = $state("");

  // SFTP — solo autenticazione a password per ora (niente selettore di
  // file per una chiave privata, coerente con lo scope "credenziali
  // statiche" delle altre card non-OAuth).
  let sftpHost = $state("");
  let sftpUser = $state("");
  let sftpPort = $state("");
  let sftpPass = $state("");

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

  // Elenco delle domande da mostrare una alla volta per i provider "a
  // credenziali statiche" (non OAuth) — stesso spirito step-by-step già
  // adottato per dropbox/onedrive, qui semplicemente scandendo un elenco
  // fisso di campi noti in anticipo invece che le domande dinamiche di
  // rclone. `required` governa solo se il pulsante "Avanti" resta
  // disabilitato finché il campo non è compilato — un campo facoltativo può
  // sempre essere saltato vuoto.
  type FieldStep = { id: string; required: boolean };
  function fieldStepsFor(k: ProviderKind | null): FieldStep[] {
    if (k === "s3") {
      return [
        { id: "provider", required: false },
        { id: "endpoint", required: false },
        { id: "accessKeyId", required: true },
        { id: "secretAccessKey", required: true },
        { id: "region", required: false },
      ];
    }
    if (k === "b2") return [{ id: "account", required: true }, { id: "key", required: true }];
    if (k === "mega") {
      return [
        { id: "user", required: true },
        { id: "pass", required: true },
        { id: "twoFa", required: false },
      ];
    }
    if (k === "nextcloud") {
      // L'indirizzo WebDAV di Nextcloud include il nome utente nel
      // percorso (/remote.php/dav/files/<utente>/): a differenza di
      // ownCloud, qui l'utente è indispensabile per comporlo, non solo per
      // autenticarsi.
      return [
        { id: "host", required: true },
        { id: "user", required: true },
        { id: "pass", required: false },
      ];
    }
    if (k === "owncloud") {
      return [
        { id: "host", required: true },
        { id: "user", required: false },
        { id: "pass", required: false },
      ];
    }
    if (k === "webdav") {
      // Nessuna convenzione di percorso nota per un servizio WebDAV
      // generico: resta un indirizzo libero, non componibile da host+utente
      // come per nextcloud/owncloud.
      return [
        { id: "url", required: true },
        { id: "user", required: false },
        { id: "pass", required: false },
      ];
    }
    if (k === "sftp") {
      return [
        { id: "host", required: true },
        { id: "user", required: false },
        { id: "port", required: false },
        { id: "pass", required: false },
      ];
    }
    return [];
  }

  let fieldIndex = $state(0);
  let fieldSteps = $derived(fieldStepsFor(kind));
  let currentField = $derived(fieldSteps[fieldIndex] ?? null);
  let isLastField = $derived(fieldIndex === fieldSteps.length - 1);

  function isCurrentFieldFilled(): boolean {
    if (!currentField) return true;
    if (kind === "s3") {
      if (currentField.id === "accessKeyId") return s3AccessKeyId.trim() !== "";
      if (currentField.id === "secretAccessKey") return s3SecretAccessKey.trim() !== "";
    } else if (kind === "b2") {
      if (currentField.id === "account") return b2Account.trim() !== "";
      if (currentField.id === "key") return b2Key.trim() !== "";
    } else if (kind === "mega") {
      if (currentField.id === "user") return megaUser.trim() !== "";
      if (currentField.id === "pass") return megaPass.trim() !== "";
    } else if (kind === "nextcloud" || kind === "owncloud") {
      if (currentField.id === "host") return webdavHost.trim() !== "";
      if (currentField.id === "user") return webdavUser.trim() !== ""; // required solo per nextcloud, vedi fieldStepsFor
    } else if (kind === "webdav") {
      if (currentField.id === "url") return webdavUrl.trim() !== "";
    } else if (kind === "sftp") {
      if (currentField.id === "host") return sftpHost.trim() !== "";
    }
    return true;
  }

  // Il nome resta sempre visibile insieme al campo corrente (come già per
  // dropbox/onedrive) invece di essere un passo a parte: è compilato una
  // volta sola, non ha senso "avanzare" oltre.
  let canGoNextField = $derived(name.trim() !== "" && (!currentField?.required || isCurrentFieldFilled()));

  function selectKind(k: ProviderKind) {
    kind = k;
    step = 2;
    fieldIndex = 0;
    errorMessage = null;
  }

  function backToStep1() {
    step = 1;
    errorMessage = null;
  }

  function goNextField() {
    if (!canGoNextField) return;
    fieldIndex++;
  }

  function goBackField() {
    if (fieldIndex > 0) fieldIndex--;
    else backToStep1();
  }

  function handleStaticFormSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (isLastField) submit();
    else goNextField();
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
      invoke<S3EndpointOption[]>("list_s3_endpoints")
        .then((endpoints) => (s3Endpoints = endpoints))
        .catch(() => {});
    }
  });

  // Solo le regioni/endpoint pertinenti al provider scelto (rclone associa
  // ogni esempio a uno o più provider) — "Other" non ne ha di noti in
  // anticipo, restano campi liberi in quel caso.
  let s3RegionsForProvider = $derived(s3Regions.filter((r) => r.providers.includes(s3Provider)));
  let s3EndpointsForProvider = $derived(s3Endpoints.filter((e) => e.providers.includes(s3Provider)));

  // Quando cambia il provider (o arriva l'elenco dal backend), propone in
  // automatico il primo endpoint e la prima regione note per quel provider
  // — l'utente resta libero di cambiarli, sono campi di testo liberi, non
  // scelte vincolate. Questi due effetti dipendono SOLO da
  // `s3EndpointsForProvider`/`s3RegionsForProvider` (a loro volta derivati
  // da `s3Provider`+l'elenco caricato), mai dal valore attuale del campo:
  // leggerlo per un confronto ("è ancora tra quelli noti?") lo renderebbe
  // una dipendenza reattiva anch'esso, e ogni tasto premuto dall'utente
  // farebbe scattare di nuovo l'effetto, sovrascrivendo la modifica appena
  // fatta con il default — bug reale (19/8/2026), il campo sembrava
  // bloccato/non modificabile.
  $effect(() => {
    s3Endpoint = s3EndpointsForProvider.length > 0 ? s3EndpointsForProvider[0].value : "";
  });

  $effect(() => {
    s3Region = s3RegionsForProvider.length > 0 ? s3RegionsForProvider[0].value : "";
  });

  // Solo l'indirizzo del server, mai il percorso WebDAV completo: toglie
  // https:// ed eventuali "/" finali digitati per abitudine, così l'utente
  // può scrivere sia "cloud.esempio.it" sia "https://cloud.esempio.it/"
  // indifferentemente.
  function normalizeHost(raw: string): string {
    return raw.trim().replace(/^https?:\/\//i, "").replace(/\/+$/, "");
  }

  $effect(() => {
    // Nextcloud e ownCloud hanno un percorso WebDAV fisso e noto (diverso
    // tra i due, vedi fieldStepsFor): componendolo da soli invece di
    // chiederlo all'utente si evita l'errore più comune di questa card
    // (percorso sbagliato/dimenticato). WebDAV generico non ha una
    // convenzione nota, il suo campo url resta libero e non viene toccato
    // qui.
    const host = normalizeHost(webdavHost);
    if (kind === "nextcloud") {
      webdavUrl = host && webdavUser.trim() ? `https://${host}/remote.php/dav/files/${encodeURIComponent(webdavUser.trim())}/` : "";
    } else if (kind === "owncloud") {
      webdavUrl = host ? `https://${host}/remote.php/webdav/` : "";
    }
  });

  let canSubmit = $derived.by(() => {
    if (name.trim() === "") return false;
    if (kind === "s3") return s3AccessKeyId.trim() !== "" && s3SecretAccessKey.trim() !== "";
    if (kind === "b2") return b2Account.trim() !== "" && b2Key.trim() !== "";
    if (kind === "mega") return megaUser.trim() !== "" && megaPass.trim() !== "";
    if (kind === "drive") return (driveClientId.trim() === "") === (driveClientSecret.trim() === "");
    // Solo il campo che rclone stesso segna come obbligatorio per questi
    // backend (url/host) — utente/password restano facoltativi, alcuni
    // WebDAV pubblici o server SFTP con ssh-agent non li richiedono.
    if (kind === "nextcloud" || kind === "owncloud" || kind === "webdav") return webdavUrl.trim() !== "";
    if (kind === "sftp") return sftpHost.trim() !== "";
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
    } else if (kind === "nextcloud" || kind === "owncloud" || kind === "webdav") {
      add("url", webdavUrl);
      add("user", webdavUser);
      add("pass", webdavPass);
      if (kind === "nextcloud") params.vendor = "nextcloud";
      else if (kind === "owncloud") params.vendor = "owncloud";
    } else if (kind === "sftp") {
      add("host", sftpHost);
      add("user", sftpUser);
      add("port", sftpPort);
      add("pass", sftpPass);
    }
    return params;
  }

  // Deve restare allineato a VERIFY_CONNECTION_TIMEOUT in remotes.rs (20s):
  // qui serve solo per il countdown mostrato all'utente, il limite vero è
  // imposto lato backend.
  const VERIFY_CONNECTION_TIMEOUT_SECONDS = 20;
  let verifyCountdown = $state<number | null>(null);

  async function submit() {
    if (!kind || !canSubmit || submitting) return;
    submitting = true;
    errorMessage = null;
    verifyCountdown = VERIFY_CONNECTION_TIMEOUT_SECONDS;
    const countdownTimer = setInterval(() => {
      if (verifyCountdown !== null && verifyCountdown > 0) verifyCountdown -= 1;
    }, 1000);
    try {
      const verifyPath = kind === "s3" ? s3Bucket.trim() : "";
      await invoke("create_remote", { name: name.trim(), kind: rcloneKindFor(kind), parameters: buildParameters(), verifyPath });
      step = 3;
    } catch (error) {
      errorMessage = String(error);
    } finally {
      submitting = false;
      verifyCountdown = null;
      clearInterval(countdownTimer);
    }
  }

  async function submitOAuth() {
    if (!kind || !canSubmit || submitting) return;
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
      // `guided: true` fa risolvere in autonomia al backend alcune domande
      // tecniche note di questo percorso curato (es. il tipo di account
      // OneDrive, e il recupero automatico se Microsoft non riesce a
      // elencare i drive disponibili — vedi interactive_remote.rs). Le
      // domande davvero impreviste restano comunque senza risposta
      // precompilata: il backend mette in pausa il flusso e arrivano qui
      // come evento rclone-easy://oauth-question, gestito sotto.
      const hasOwnClientId = kind === "drive" && driveClientId.trim() !== "" && driveClientSecret.trim() !== "";
      await invoke("create_remote_interactive", {
        name: name.trim(),
        kind,
        extraAnswers: {},
        parameters: hasOwnClientId ? { client_id: driveClientId.trim(), client_secret: driveClientSecret.trim() } : {},
        guided: true,
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
    if (oauthUrl) await invoke("open_url_in_browser", { url: oauthUrl });
  }

  async function openGoogleDriveClientIdGuide() {
    await invoke("open_url_in_browser", { url: "https://rclone.org/drive/#making-your-own-client-id" });
  }

  async function openGoogleCloudConsole() {
    await invoke("open_url_in_browser", { url: "https://console.cloud.google.com/" });
  }

  // Elenco piatto (non raggruppato per tipo di autenticazione) e ordinato
  // alfabeticamente sul nome mostrato — un utente scorre cercando il nome
  // del servizio, non sa/non deve sapere se dietro c'è OAuth o una
  // password. Ricalcolato ad ogni cambio di lingua ($t già reattivo).
  let providerList = $derived.by<{ kind: ProviderKind; label: string; desc: string }[]>(() => {
    const items: { kind: ProviderKind; label: string; desc: string }[] = [
      { kind: "s3", label: $t("newRemote.s3Card"), desc: $t("newRemote.s3CardDesc") },
      { kind: "b2", label: "Backblaze B2", desc: $t("newRemote.b2CardDesc") },
      { kind: "mega", label: "MEGA", desc: $t("newRemote.megaCardDesc") },
      { kind: "drive", label: "Google Drive", desc: $t("newRemote.driveCardDesc") },
      { kind: "dropbox", label: "Dropbox", desc: $t("newRemote.oauthCardDesc") },
      { kind: "onedrive", label: "OneDrive", desc: $t("newRemote.oauthCardDesc") },
      { kind: "nextcloud", label: "Nextcloud", desc: $t("newRemote.nextcloudCardDesc") },
      { kind: "owncloud", label: "ownCloud", desc: $t("newRemote.owncloudCardDesc") },
      { kind: "webdav", label: "WebDAV", desc: $t("newRemote.webdavCardDesc") },
      { kind: "sftp", label: "SFTP", desc: $t("newRemote.sftpCardDesc") },
    ];
    return items.sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: "base" }));
  });

  // Nome "umano" del provider scelto, per il titolo della pagina una volta
  // superato lo step 1 (es. "Aggiungi Dropbox" invece del generico
  // "Aggiungi un remote") — preso dalla stessa lista mostrata in griglia,
  // niente di duplicato altrove.
  let selectedProviderLabel = $derived(providerList.find((p) => p.kind === kind)?.label ?? "");
</script>

<main class="container">
  <h1 class="page-title">
    {#if kind && step !== 1}
      <span class="page-title-icon"><ProviderIcon kind={kind} /></span>
      {$t("newRemote.addTitleFor", { values: { provider: selectedProviderLabel } })}
    {:else}
      {$t("newRemote.title")}
    {/if}
  </h1>

  {#if step === 1}
    <a href="/" class="back-link">← {$t("newRemote.backToHome")}</a>
    <p class="subtitle">{$t("newRemote.subtitle")}</p>
    <!-- Prima riga, due sole colonne: i due percorsi "non è uno dei servizi
         qui sotto" (importa una config rclone già esistente, o segui la
         procedura guidata di rclone per un backend qualunque) — stesso peso
         visivo tra loro, distinto dalla griglia dei servizi curati sotto. -->
    <div class="entry-row">
      <button class="card entry-card" onclick={() => goto("/importa-remote")}>
        <strong>{$t("home.importExistingRemotes")}</strong>
        <span class="card-desc">{$t("newRemote.importCardDesc")}</span>
      </button>
      <button class="card entry-card" onclick={() => goto("/nuovo-remote-avanzato")}>
        <strong>{$t("newRemote.advancedCardTitle")}</strong>
        <span class="card-desc">{$t("newRemote.advancedCardDesc")}</span>
      </button>
    </div>
    <!-- Griglia dei servizi curati, unica parte scorrevole della pagina:
         può crescere (oggi 10 servizi, di più in futuro) senza costringere
         a scorrere l'intera schermata solo per arrivare alla riga sopra o
         al link "Torna alla home" — quelli restano sempre visibili. -->
    <div class="provider-grid">
      {#each providerList as p (p.kind)}
        <button class="tile" onclick={() => selectKind(p.kind)} title={p.desc}>
          <ProviderIcon kind={p.kind} />
          <span class="tile-label">{p.label}</span>
        </button>
      {/each}
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
    <form onsubmit={handleStaticFormSubmit}>
      <label>
        {$t("newRemote.remoteNameLabel")}
        <input type="text" bind:value={name} placeholder={$t("newRemote.remoteNamePlaceholder")} />
      </label>

      {#if fieldSteps.length > 0}
        <span class="step-progress">{$t("advancedRemote.stepProgress")} {fieldIndex + 1}/{fieldSteps.length}</span>
      {/if}

      {#if kind === "s3" && currentField?.id === "provider"}
        <label>
          Provider
          <select bind:value={s3Provider}>
            <option value="Other">{$t("newRemote.otherCustomEndpoint")}</option>
            {#each s3Providers as p (p.value)}
              <option value={p.value} title={p.help}>{p.value}</option>
            {/each}
          </select>
        </label>
      {:else if kind === "s3" && currentField?.id === "endpoint"}
        <label>
          {$t("newRemote.endpointLabel")}
          <input
            type="text"
            bind:value={s3Endpoint}
            list="s3-endpoint-suggestions"
            placeholder="es. s3.cubbit.eu, s3.wasabisys.com"
          />
        </label>
        {#if s3EndpointsForProvider.length > 0}
          <!-- Suggerimento, non un vincolo: un provider può avere account
               (es. Cubbit in prova) che richiedono un endpoint diverso da
               quello proposto qui — un elenco a scelta fissa impedirebbe di
               scriverlo. -->
          <datalist id="s3-endpoint-suggestions">
            {#each s3EndpointsForProvider as e (e.value)}
              <option value={e.value}>{e.help}</option>
            {/each}
          </datalist>
        {/if}
      {:else if kind === "s3" && currentField?.id === "accessKeyId"}
        <label>
          Access Key ID
          <input type="text" bind:value={s3AccessKeyId} />
        </label>
      {:else if kind === "s3" && currentField?.id === "secretAccessKey"}
        <label>
          Secret Access Key
          <input type="password" bind:value={s3SecretAccessKey} />
        </label>
      {:else if kind === "s3" && currentField?.id === "region"}
        <label>
          {$t("newRemote.regionLabel")}
          <input type="text" bind:value={s3Region} list="s3-region-suggestions" placeholder="es. eu-west-1" />
        </label>
        {#if s3RegionsForProvider.length > 0}
          <datalist id="s3-region-suggestions">
            {#each s3RegionsForProvider as r (r.value)}
              <option value={r.value}>{r.help}</option>
            {/each}
          </datalist>
        {/if}
      {:else if kind === "b2" && currentField?.id === "account"}
        <label>
          Account ID
          <input type="text" bind:value={b2Account} />
        </label>
      {:else if kind === "b2" && currentField?.id === "key"}
        <label>
          Application Key
          <input type="password" bind:value={b2Key} />
        </label>
      {:else if kind === "mega" && currentField?.id === "user"}
        <label>
          {$t("newRemote.emailLabel")}
          <input type="text" bind:value={megaUser} />
        </label>
      {:else if kind === "mega" && currentField?.id === "pass"}
        <label>
          {$t("unlock.passwordLabel")}
          <input type="password" bind:value={megaPass} />
        </label>
      {:else if kind === "mega" && currentField?.id === "twoFa"}
        <label>
          {$t("newRemote.twoFaLabel")}
          <input type="text" bind:value={megaTwoFa} />
        </label>
      {:else if (kind === "nextcloud" || kind === "owncloud") && currentField?.id === "host"}
        <label>
          {$t("newRemote.hostLabel")}
          <input type="text" bind:value={webdavHost} placeholder="es. cloud.esempio.it" />
        </label>
        <p class="hint">{$t("newRemote.webdavHostHint")}</p>
        {#if kind === "owncloud" && webdavUrl}
          <p class="hint">{$t("newRemote.composedUrlHint", { values: { url: webdavUrl } })}</p>
        {/if}
      {:else if kind === "webdav" && currentField?.id === "url"}
        <label>
          {$t("newRemote.urlLabel")}
          <input type="text" bind:value={webdavUrl} placeholder="https://esempio.it/webdav/" />
        </label>
      {:else if (kind === "nextcloud" || kind === "owncloud" || kind === "webdav") && currentField?.id === "user"}
        <label>
          {$t("newRemote.usernameLabel")}
          <input type="text" bind:value={webdavUser} />
        </label>
        {#if kind === "nextcloud" && webdavUrl}
          <p class="hint">{$t("newRemote.composedUrlHint", { values: { url: webdavUrl } })}</p>
        {/if}
      {:else if (kind === "nextcloud" || kind === "owncloud" || kind === "webdav") && currentField?.id === "pass"}
        <label>
          {$t("unlock.passwordLabel")}
          <input type="password" bind:value={webdavPass} />
        </label>
        {#if kind === "nextcloud" || kind === "owncloud"}
          <p class="hint">{$t("newRemote.nextcloud2faHint")}</p>
        {/if}
      {:else if kind === "sftp" && currentField?.id === "host"}
        <label>
          {$t("newRemote.hostLabel")}
          <input type="text" bind:value={sftpHost} placeholder="es. example.com" />
        </label>
      {:else if kind === "sftp" && currentField?.id === "user"}
        <label>
          {$t("newRemote.usernameLabel")}
          <input type="text" bind:value={sftpUser} />
        </label>
      {:else if kind === "sftp" && currentField?.id === "port"}
        <label>
          {$t("newRemote.portLabel")}
          <input type="text" bind:value={sftpPort} placeholder="22" />
        </label>
      {:else if kind === "sftp" && currentField?.id === "pass"}
        <label>
          {$t("unlock.passwordLabel")}
          <input type="password" bind:value={sftpPass} />
        </label>
      {/if}

      {#if errorMessage}
        <p class="error">✗ {$t("newRemote.connectionFailed", { values: { error: errorMessage } })}</p>
        {#if kind === "s3"}
          <!-- Non chiesto in anticipo a tutti (la maggior parte delle
               chiavi S3 ha accesso a tutto l'account): compare solo dopo un
               primo tentativo fallito, così chi non ne ha bisogno non lo
               vede mai — stesso principio del recupero automatico già
               usato per OneDrive. -->
          <label>
            {$t("newRemote.bucketRetryLabel")}
            <input type="text" bind:value={s3Bucket} placeholder="es. nome-bucket" />
          </label>
        {/if}
      {/if}

      {#if verifyCountdown !== null}
        <p class="hint">{$t("newRemote.verifyingConnectionCountdown", { values: { seconds: verifyCountdown } })}</p>
      {/if}

      <div class="actions">
        <button type="button" onclick={goBackField} disabled={submitting}>{$t("newRemote.back")}</button>
        <button type="submit" disabled={!canGoNextField || submitting}>
          {#if !isLastField}
            {$t("advancedRemote.next")}
          {:else}
            {submitting ? $t("importPanel.checking") : $t("newRemote.verifyAndSave")}
          {/if}
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

.page-title {
  display: flex;
  align-items: center;
  gap: 0.4em;
}

/* ProviderIcon dimensiona sé stessa in em rispetto al proprio font-size —
   relativo a quello dell'h1 (non un valore fisso) così l'icona resta
   proporzionata al titolo qualunque sia la sua dimensione effettiva. */
.page-title-icon {
  display: inline-flex;
  font-size: 0.7em;
}

.subtitle {
  color: var(--text-muted);
  margin-top: 1em;
}

.back-link {
  display: inline-block;
  color: var(--accent);
}

.card {
  display: flex;
  flex-direction: column;
  gap: 0.25em;
  width: 100%;
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

.card-desc {
  color: var(--text-muted);
  font-size: 0.9em;
}

.entry-row {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 0.6em;
  margin-top: 1.5em;
}

.entry-card {
  height: 100%;
}

/* Griglia dei servizi curati: unica zona scorrevole della pagina (vedi il
   commento nel markup) — altezza massima tarata per lasciare comunque
   intravedere che c'è altro sotto senza dover indovinare, il resto della
   pagina (titolo, link, riga "Importa"/"Procedura guidata") resta sempre
   fisso in vista. Più colonne di quante ne stiano larghe (auto-fill): su
   schermi stretti degrada a poche colonne invece di restare illeggibile. */
.provider-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(6.5em, 1fr));
  gap: 0.6em;
  margin-top: 0.9em;
  max-height: 55vh;
  overflow-y: auto;
  padding-right: 0.3em;
}

.tile {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.5em;
  padding: 1em 0.6em;
  border-radius: 8px;
  border: 1px solid transparent;
  background-color: var(--bg-surface);
  box-shadow: var(--shadow-md);
  cursor: pointer;
  font-family: inherit;
  color: inherit;
  text-align: center;
}

.tile-label {
  font-size: 0.85em;
  font-weight: 600;
}

.step-progress {
  color: var(--text-muted);
  font-size: 0.85em;
  text-transform: uppercase;
  letter-spacing: 0.03em;
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
