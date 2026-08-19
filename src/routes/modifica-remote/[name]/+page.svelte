<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { goto } from "$app/navigation";
  import { page } from "$app/stores";
  import { untrack } from "svelte";
  import { t } from "$lib/i18n";
  import ProviderIcon from "$lib/components/ProviderIcon.svelte";

  type RemoteForEdit = { kind: string; parameters: Record<string, string> };
  type S3ProviderOption = { value: string; help: string };
  type S3RegionOption = { value: string; help: string; providers: string[] };
  type S3EndpointOption = { value: string; help: string; providers: string[] };
  type LoadState = { status: "loading" } | { status: "ok" } | { status: "error"; message: string };

  const remoteName = $derived(decodeURIComponent($page.params.name ?? ""));

  // Copia iniziale, non un valore reattivo legato a `remoteName`: è il
  // campo che l'utente modifica per rinominare, non deve risincronizzarsi
  // da solo se `remoteName` cambiasse (non dovrebbe succedere finché questa
  // pagina resta aperta sullo stesso remote, ma `untrack` lo rende esplicito
  // invece di affidarsi a un dettaglio implementativo).
  let newName = $state(untrack(() => remoteName));
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
  // Non è un campo del backend s3 (il bucket fa sempre parte del percorso
  // quando si usa il remote, mai della sua config) — serve solo a
  // verificare la connessione su un bucket specifico invece che
  // sull'intero account, stesso principio di nuovo-remote.
  let s3Bucket = $state("");
  let s3Providers = $state<S3ProviderOption[]>([]);
  let s3Regions = $state<S3RegionOption[]>([]);
  let s3Endpoints = $state<S3EndpointOption[]>([]);
  // Solo un cambio di provider fatto dall'utente deve poter proporre un
  // nuovo endpoint/regione di default — al caricamento iniziale i valori
  // già salvati vanno sempre rispettati anche se non compaiono tra gli
  // esempi noti di rclone (endpoint personalizzati possono avere
  // regioni/endpoint non catalogati).
  let s3ProviderTouchedByUser = $state(false);

  // Backblaze B2
  let b2Account = $state("");
  let b2Key = $state("");

  // MEGA
  let megaUser = $state("");
  let megaPass = $state("");

  // Nextcloud/ownCloud/WebDAV sono tutti "webdav" lato rclone (vedi
  // remotes.rs::non_secret_fields_for) — un solo campo "vendor" li
  // distingue. Per nextcloud/owncloud mostriamo lo stesso campo Host
  // semplificato del wizard di creazione (vedi `webdavHost` sotto),
  // ricostruito dall'URL già salvato; per qualunque altro vendor (o
  // nessuno) restano i campi grezzi Vendor+URL, non essendoci una
  // convenzione di percorso nota da poter comporre automaticamente.
  let webdavVendor = $state("");
  let webdavHost = $state("");
  let webdavUrl = $state("");
  let webdavUser = $state("");
  let webdavPass = $state("");

  // SFTP
  let sftpHost = $state("");
  let sftpUser = $state("");
  let sftpPort = $state("");
  let sftpPass = $state("");

  // Google Drive: nessun campo modificabile qui (vedi il banner sotto), solo
  // se ha già un client_id proprio o usa ancora l'identità condivisa che
  // Google ritirerà nel corso del 2026 — `client_id` è l'unico campo che
  // `get_remote_for_edit` espone per "drive", vedi remotes.rs.
  let driveHasOwnClientId = $state(false);

  // Ricostruisce il campo Host semplificato (vedi nuovo-remote) da un URL
  // WebDAV già salvato, per nextcloud/owncloud creati con quella stessa
  // convenzione — solo un tentativo migliore possibile: un URL che non
  // segue esattamente lo schema atteso (es. un remote nextcloud importato
  // da fuori, o modificato a mano fuori dall'app) lascia semplicemente
  // dentro qualche frammento di percorso in più, comunque un campo di
  // testo libero e modificabile, non un errore bloccante.
  function hostFromComposedUrl(vendor: string, url: string): string {
    const stripped = url.trim().replace(/^https?:\/\//i, "");
    if (vendor === "nextcloud") return stripped.replace(/\/remote\.php\/dav\/files\/[^/]*\/?$/i, "");
    if (vendor === "owncloud") return stripped.replace(/\/remote\.php\/webdav\/?$/i, "");
    return stripped;
  }

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
          invoke<S3EndpointOption[]>("list_s3_endpoints")
            .then((endpoints) => (s3Endpoints = endpoints))
            .catch(() => {});
        } else if (kind === "b2") {
          b2Account = result.parameters.account ?? "";
        } else if (kind === "mega") {
          megaUser = result.parameters.user ?? "";
        } else if (kind === "drive") {
          driveHasOwnClientId = (result.parameters.client_id ?? "").trim() !== "";
        } else if (kind === "webdav") {
          webdavVendor = result.parameters.vendor ?? "";
          webdavUrl = result.parameters.url ?? "";
          webdavUser = result.parameters.user ?? "";
          if (webdavVendor === "nextcloud" || webdavVendor === "owncloud") {
            webdavHost = hostFromComposedUrl(webdavVendor, webdavUrl);
          }
        } else if (kind === "sftp") {
          sftpHost = result.parameters.host ?? "";
          sftpUser = result.parameters.user ?? "";
          sftpPort = result.parameters.port ?? "";
        }
        loadState = { status: "ok" };
      })
      .catch((error) => (loadState = { status: "error", message: String(error) }));
  });

  let s3RegionsForProvider = $derived(s3Regions.filter((r) => r.providers.includes(s3Provider)));
  let s3EndpointsForProvider = $derived(s3Endpoints.filter((e) => e.providers.includes(s3Provider)));

  // Solo quando l'utente CAMBIA provider (mai al caricamento, vedi sopra) si
  // propone un nuovo endpoint/regione di default — questi due effetti
  // dipendono solo da `s3ProviderTouchedByUser` e
  // dagli elenchi per provider, mai dal valore attuale del campo: leggerlo
  // per un confronto lo renderebbe una dipendenza reattiva anch'esso, e ogni
  // tasto premuto dall'utente farebbe scattare di nuovo l'effetto
  // sovrascrivendo la modifica appena fatta — stesso bug reale già trovato
  // e corretto in nuovo-remote (19/8/2026).
  $effect(() => {
    if (!s3ProviderTouchedByUser) return;
    s3Region = s3RegionsForProvider.length > 0 ? s3RegionsForProvider[0].value : "";
  });

  $effect(() => {
    if (!s3ProviderTouchedByUser) return;
    s3Endpoint = s3EndpointsForProvider.length > 0 ? s3EndpointsForProvider[0].value : "";
  });

  // Stesso principio del wizard di creazione: solo per nextcloud/owncloud,
  // compone da solo l'URL WebDAV da Host+Utente (owncloud non ha bisogno
  // dell'utente nel percorso) — mai per altri vendor, dove l'URL resta un
  // campo libero senza convenzione nota.
  function normalizeHost(raw: string): string {
    return raw.trim().replace(/^https?:\/\//i, "").replace(/\/+$/, "");
  }

  $effect(() => {
    const host = normalizeHost(webdavHost);
    if (webdavVendor === "nextcloud") {
      webdavUrl = host && webdavUser.trim() ? `https://${host}/remote.php/dav/files/${encodeURIComponent(webdavUser.trim())}/` : "";
    } else if (webdavVendor === "owncloud") {
      webdavUrl = host ? `https://${host}/remote.php/webdav/` : "";
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
    } else if (kind === "webdav") {
      add("vendor", webdavVendor);
      add("url", webdavUrl);
      add("user", webdavUser);
      add("pass", webdavPass);
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
    const trimmedName = newName.trim();
    if (trimmedName === "") return;
    submitting = true;
    errorMessage = null;
    verifyCountdown = VERIFY_CONNECTION_TIMEOUT_SECONDS;
    const countdownTimer = setInterval(() => {
      if (verifyCountdown !== null && verifyCountdown > 0) verifyCountdown -= 1;
    }, 1000);
    try {
      const verifyPath = kind === "s3" ? s3Bucket.trim() : "";
      await invoke("update_remote", { oldName: remoteName, name: trimmedName, parameters: buildParameters(), verifyPath });
      goto("/");
    } catch (error) {
      errorMessage = String(error);
    } finally {
      submitting = false;
      verifyCountdown = null;
      clearInterval(countdownTimer);
    }
  }

  // Chiave per ProviderIcon: coincide con `kind` per la maggior parte dei
  // backend, tranne "webdav" che in ProviderIcon è già distinto per vendor
  // (nextcloud/owncloud/webdav) — stessa mappa concettuale delle card nel
  // wizard di creazione.
  let providerIconKind = $derived.by(() => {
    if (kind === "webdav") {
      if (webdavVendor === "nextcloud") return "nextcloud";
      if (webdavVendor === "owncloud") return "owncloud";
      return "webdav";
    }
    return kind;
  });
</script>

<main class="container">
  <a href="/" class="back-link">← {$t("newRemote.backToHome")}</a>
  <h1 class="page-title">
    {#if loadState.status === "ok"}
      <span class="page-title-icon"><ProviderIcon kind={providerIconKind} /></span>
    {/if}
    {$t("editRemote.title", { values: { remote: remoteName } })}
  </h1>

  {#if loadState.status === "loading"}
    <p>{$t("common.loading")}</p>
  {:else if loadState.status === "error"}
    <p class="error">✗ {loadState.message}</p>
  {:else}
    {#if kind === "drive" && !driveHasOwnClientId}
      <p class="warning">
        {$t("editRemote.driveSharedWarningBefore")} <strong>{$t("editRemote.driveSharedWarningStrong")}</strong>{$t("editRemote.driveSharedWarningAfter", { values: { remote: remoteName } })}
      </p>
    {:else if kind === "drive"}
      <p class="ok">✓ {$t("editRemote.driveOwnClientOk")}</p>
    {:else if kind === "dropbox" || kind === "onedrive"}
      <p class="warning">
        {$t("editRemote.oauthNotEditable")}
      </p>
    {/if}
    <form onsubmit={(e) => { e.preventDefault(); submit(); }}>
      <label>
        {$t("editRemote.nameLabel")}
        <input type="text" bind:value={newName} required />
      </label>
      {#if kind === "s3"}
        <label>
          Provider
          <select bind:value={s3Provider} onchange={() => (s3ProviderTouchedByUser = true)}>
            <option value="Other">{$t("newRemote.otherCustomEndpoint")}</option>
            {#each s3Providers as p (p.value)}
              <option value={p.value} title={p.help}>{p.value}</option>
            {/each}
          </select>
        </label>
        <label>
          {$t("newRemote.endpointLabel")}
          <input type="text" bind:value={s3Endpoint} list="s3-endpoint-suggestions" placeholder="es. s3.cubbit.eu, s3.wasabisys.com" />
        </label>
        {#if s3EndpointsForProvider.length > 0}
          <!-- Suggerimento, non un vincolo: alcuni account (es. Cubbit in
               prova) richiedono un endpoint diverso da quello proposto. -->
          <datalist id="s3-endpoint-suggestions">
            {#each s3EndpointsForProvider as e (e.value)}
              <option value={e.value}>{e.help}</option>
            {/each}
          </datalist>
        {/if}
        <label>
          Access Key ID
          <input type="text" bind:value={s3AccessKeyId} />
        </label>
        <label>
          Secret Access Key
          <input type="password" bind:value={s3SecretAccessKey} placeholder={$t("editRemote.leaveEmptyToKeep")} />
        </label>
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
      {:else if kind === "b2"}
        <label>
          Account ID
          <input type="text" bind:value={b2Account} />
        </label>
        <label>
          Application Key
          <input type="password" bind:value={b2Key} placeholder={$t("editRemote.leaveEmptyToKeep")} />
        </label>
      {:else if kind === "mega"}
        <label>
          Email
          <input type="text" bind:value={megaUser} />
        </label>
        <label>
          {$t("unlock.passwordLabel")}
          <input type="password" bind:value={megaPass} placeholder={$t("editRemote.leaveEmptyToKeep")} />
        </label>
      {:else if kind === "webdav" && (webdavVendor === "nextcloud" || webdavVendor === "owncloud")}
        <!-- Stesso principio del wizard di creazione: solo Host, l'URL
             WebDAV completo si compone da solo (vedi l'effetto sopra). -->
        <label>
          {$t("newRemote.hostLabel")}
          <input type="text" bind:value={webdavHost} placeholder="es. cloud.esempio.it" />
        </label>
        <p class="hint">{$t("newRemote.webdavHostHint")}</p>
        {#if webdavUrl}
          <p class="hint">{$t("newRemote.composedUrlHint", { values: { url: webdavUrl } })}</p>
        {/if}
        <label>
          {$t("newRemote.usernameLabel")}
          <input type="text" bind:value={webdavUser} />
        </label>
        <label>
          {$t("unlock.passwordLabel")}
          <input type="password" bind:value={webdavPass} placeholder={$t("editRemote.leaveEmptyToKeep")} />
        </label>
        {#if webdavVendor === "nextcloud" || webdavVendor === "owncloud"}
          <p class="hint">{$t("newRemote.nextcloud2faHint")}</p>
        {/if}
      {:else if kind === "webdav"}
        <label>
          Vendor
          <input type="text" bind:value={webdavVendor} placeholder="nextcloud, owncloud, other…" />
        </label>
        <label>
          {$t("newRemote.urlLabel")}
          <input type="text" bind:value={webdavUrl} />
        </label>
        <label>
          {$t("newRemote.usernameLabel")}
          <input type="text" bind:value={webdavUser} />
        </label>
        <label>
          {$t("unlock.passwordLabel")}
          <input type="password" bind:value={webdavPass} placeholder={$t("editRemote.leaveEmptyToKeep")} />
        </label>
      {:else if kind === "sftp"}
        <label>
          {$t("newRemote.hostLabel")}
          <input type="text" bind:value={sftpHost} />
        </label>
        <label>
          {$t("newRemote.usernameLabel")}
          <input type="text" bind:value={sftpUser} />
        </label>
        <label>
          {$t("newRemote.portLabel")}
          <input type="text" bind:value={sftpPort} placeholder="22" />
        </label>
        <label>
          {$t("unlock.passwordLabel")}
          <input type="password" bind:value={sftpPass} placeholder={$t("editRemote.leaveEmptyToKeep")} />
        </label>
      {/if}

      {#if errorMessage}
        <p class="error">✗ {$t("newRemote.connectionFailed", { values: { error: errorMessage } })}</p>
        {#if kind === "s3"}
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
        <button type="submit" disabled={submitting}>
          {submitting ? $t("importPanel.checking") : $t("newRemote.verifyAndSave")}
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

.page-title {
  display: flex;
  align-items: center;
  gap: 0.4em;
}

.page-title-icon {
  display: inline-flex;
  font-size: 0.7em;
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
