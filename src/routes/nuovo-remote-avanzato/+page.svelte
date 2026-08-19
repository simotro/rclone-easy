<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { t } from "$lib/i18n";

  type ProviderTypeOption = { name: string; description: string };
  type FieldExample = { value: string; help: string };
  type ProviderFieldOption = {
    name: string;
    help: string;
    type: string;
    defaultStr: string;
    required: boolean;
    advanced: boolean;
    isPassword: boolean;
    sensitive: boolean;
    exclusive: boolean;
    examples: FieldExample[];
  };
  type OAuthQuestion = { help: string; examples: { value: string; help: string }[]; defaultValue: string };

  // Una domanda alla volta, come `rclone config` da terminale — non un
  // form lungo con tutti i campi insieme. `pick` = scelta del backend,
  // `name` = nome del remote, `field` = un campo alla volta (prima quelli
  // di base, poi eventualmente quelli avanzati), `advancedPrompt` =
  // l'equivalente del sì/no "Edit advanced config?" di rclone,
  // `done` = creato.
  type Phase = "pick" | "name" | "field" | "advancedPrompt" | "done";
  let phase = $state<Phase>("pick");

  // --- Scelta del backend: elenco completo di rclone ---
  let providerTypes = $state<ProviderTypeOption[]>([]);
  let providerTypesError = $state<string | null>(null);
  let searchQuery = $state("");

  $effect(() => {
    if (providerTypes.length === 0 && !providerTypesError) {
      invoke<ProviderTypeOption[]>("list_provider_types")
        .then((types) => (providerTypes = types.sort((a, b) => a.name.localeCompare(b.name))))
        .catch((error) => (providerTypesError = String(error)));
    }
  });

  let filteredTypes = $derived.by(() => {
    const q = searchQuery.trim().toLowerCase();
    if (q === "") return providerTypes;
    return providerTypes.filter((p) => p.name.toLowerCase().includes(q) || p.description.toLowerCase().includes(q));
  });

  let kind = $state<string | null>(null);
  let name = $state("");
  let fields = $state<ProviderFieldOption[]>([]);
  let fieldsError = $state<string | null>(null);
  let loadingFields = $state(false);
  let fieldValues = $state<Record<string, string>>({});

  let basicFields = $derived(fields.filter((f) => !f.advanced));
  let advancedFields = $derived(fields.filter((f) => f.advanced));

  let inAdvancedPhase = $state(false);
  let fieldIndex = $state(0);
  let currentField = $derived((inAdvancedPhase ? advancedFields : basicFields)[fieldIndex] ?? null);

  function isBoolField(f: ProviderFieldOption): boolean {
    return f.type === "bool" || f.type === "Tristate";
  }

  async function selectKind(k: string) {
    kind = k;
    name = "";
    fieldValues = {};
    fieldsError = null;
    loadingFields = true;
    errorMessage = null;
    phase = "name";
    try {
      fields = await invoke<ProviderFieldOption[]>("get_provider_options", { kind: k });
    } catch (error) {
      fieldsError = String(error);
      fields = [];
    } finally {
      loadingFields = false;
    }
  }

  function backToPick() {
    phase = "pick";
    kind = null;
    errorMessage = null;
  }

  // Dal nome si passa al primo campo di base, oppure — se il backend non
  // ne ha nessuno — direttamente alla domanda sulle opzioni avanzate (o al
  // salvataggio, se non ne ha nemmeno di quelle: capita, es. Mega non ha
  // campi avanzati).
  function startFields() {
    if (name.trim() === "") return;
    inAdvancedPhase = false;
    fieldIndex = 0;
    if (basicFields.length > 0) {
      phase = "field";
    } else {
      afterLastBasicField();
    }
  }

  function afterLastBasicField() {
    if (advancedFields.length > 0) {
      phase = "advancedPrompt";
    } else {
      submit();
    }
  }

  function canProceedFromCurrentField(): boolean {
    if (!currentField) return true;
    if (!currentField.required) return true;
    return (fieldValues[currentField.name] ?? "").trim() !== "";
  }

  function nextField() {
    if (!canProceedFromCurrentField()) return;
    const list = inAdvancedPhase ? advancedFields : basicFields;
    if (fieldIndex + 1 < list.length) {
      fieldIndex++;
    } else if (!inAdvancedPhase) {
      afterLastBasicField();
    } else {
      submit();
    }
  }

  function previousField() {
    if (fieldIndex > 0) {
      fieldIndex--;
      return;
    }
    if (inAdvancedPhase) {
      phase = "advancedPrompt";
    } else {
      phase = "name";
    }
  }

  function answerAdvancedPrompt(wantsAdvanced: boolean) {
    if (wantsAdvanced && advancedFields.length > 0) {
      inAdvancedPhase = true;
      fieldIndex = 0;
      phase = "field";
    } else {
      submit();
    }
  }

  function buildParameters(): Record<string, string> {
    const params: Record<string, string> = {};
    for (const f of fields) {
      const value = (fieldValues[f.name] ?? "").trim();
      if (value !== "") params[f.name] = value;
    }
    return params;
  }

  // --- Invio, con eventuale attesa di un passaggio interattivo
  //     (tipicamente OAuth) — stesso meccanismo già usato in nuovo-remote,
  //     riusato qui perché `create_remote_interactive` gestisce entrambi i
  //     casi (backend "semplice": finisce subito; backend con un passaggio
  //     speciale incorporato: continua con queste stesse domande). ---
  let submitting = $state(false);
  let errorMessage = $state<string | null>(null);
  let oauthUrl = $state<string | null>(null);
  let oauthWaiting = $state(false);
  let oauthQuestion = $state<OAuthQuestion | null>(null);
  let oauthFreeTextAnswer = $state("");

  async function submit() {
    if (!kind || submitting) return;
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
      // `guided: false`: qui l'utente deve vedere ogni domanda reale di
      // rclone così com'è, senza risoluzioni automatiche silenziose — è il
      // punto di questo percorso avanzato.
      await invoke("create_remote_interactive", {
        name: name.trim(),
        kind,
        extraAnswers: {},
        parameters: buildParameters(),
        guided: false,
      });
      phase = "done";
    } catch (error) {
      errorMessage = String(error);
      // Torna all'ultima domanda utile invece di lasciare l'utente su una
      // schermata senza campi visibili: se c'erano opzioni avanzate non
      // ancora chieste, il posto più sensato è lì.
      phase = advancedFields.length > 0 ? "advancedPrompt" : "name";
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
</script>

<main class="container">
  <h1>{$t("advancedRemote.title")}</h1>

  {#if oauthWaiting}
    <div class="oauth-wait">
      {#if oauthQuestion}
        <p>{oauthQuestion.help}</p>
        {#if oauthQuestion.examples.length > 0}
          <div class="oauth-question-choices">
            {#each oauthQuestion.examples as example (example.value)}
              <button type="button" onclick={() => answerOAuthQuestion(example.value)}>{example.help}</button>
            {/each}
          </div>
        {:else}
          <div class="oauth-question-choices">
            <input type="text" bind:value={oauthFreeTextAnswer} />
            <button type="button" onclick={() => answerOAuthQuestion(oauthFreeTextAnswer)}>{$t("common.confirm")}</button>
          </div>
        {/if}
      {:else}
        <p>{$t("advancedRemote.waitingForInteractiveStep")}</p>
        {#if oauthUrl}
          <button type="button" class="link-button" onclick={openOAuthUrlManually}>{$t("newRemote.oauthOpenLinkManually")}</button>
        {/if}
        <div class="actions">
          <button type="button" onclick={cancelOAuth}>{$t("common.cancel")}</button>
        </div>
      {/if}
    </div>
  {:else if phase === "pick"}
    <p class="subtitle">{$t("advancedRemote.subtitle")}</p>
    <a href="/nuovo-remote" class="back-link">← {$t("newRemote.back")}</a>
    <input type="text" class="search-box" bind:value={searchQuery} placeholder={$t("advancedRemote.searchPlaceholder")} />
    {#if providerTypesError}
      <p class="error">✗ {providerTypesError}</p>
    {:else if providerTypes.length === 0}
      <p class="hint">{$t("home.checking")}</p>
    {:else}
      <div class="provider-list">
        {#each filteredTypes as p (p.name)}
          <button class="card" onclick={() => selectKind(p.name)}>
            <strong>{p.name}</strong>
            <span class="card-desc">{p.description}</span>
          </button>
        {/each}
      </div>
    {/if}
  {:else if phase === "name"}
    <div class="step">
      <span class="step-progress">{kind}</span>
      <h2>{$t("newRemote.remoteNameLabel")}</h2>
      {#if loadingFields}
        <p class="hint">{$t("home.checking")}</p>
      {:else if fieldsError}
        <p class="error">✗ {fieldsError}</p>
      {:else}
        <input
          type="text"
          bind:value={name}
          placeholder={$t("newRemote.remoteNamePlaceholder")}
          onkeydown={(e) => e.key === "Enter" && startFields()}
        />
        {#if errorMessage}<p class="error">✗ {errorMessage}</p>{/if}
        <div class="actions">
          <button type="button" onclick={backToPick}>{$t("newRemote.back")}</button>
          <button type="button" onclick={startFields} disabled={name.trim() === ""}>{$t("advancedRemote.next")}</button>
        </div>
      {/if}
    </div>
  {:else if phase === "field" && currentField}
    {@const f = currentField}
    {@const list = inAdvancedPhase ? advancedFields : basicFields}
    <div class="step">
      <span class="step-progress">
        {inAdvancedPhase ? $t("advancedRemote.advancedStepProgress") : $t("advancedRemote.stepProgress")}
        {fieldIndex + 1}/{list.length}
      </span>
      <h2>{f.name}{#if f.required}<span class="required-marker"> *</span>{/if}</h2>
      {#if f.help}<p class="field-help">{f.help}</p>{/if}

      {#if isBoolField(f)}
        <label class="checkbox-row">
          <input
            type="checkbox"
            checked={fieldValues[f.name] === "true"}
            onchange={(e) => (fieldValues[f.name] = (e.currentTarget as HTMLInputElement).checked ? "true" : "false")}
          />
          {$t("advancedRemote.enableThisOption")}
        </label>
      {:else if f.isPassword || f.sensitive}
        <input type="password" bind:value={fieldValues[f.name]} placeholder={f.defaultStr} />
      {:else if f.examples.length > 0 && f.exclusive}
        <select bind:value={fieldValues[f.name]}>
          <option value="">{$t("advancedRemote.chooseValue")}</option>
          {#each f.examples as ex (ex.value)}
            <option value={ex.value} title={ex.help}>{ex.value} — {ex.help}</option>
          {/each}
        </select>
      {:else}
        <input type="text" bind:value={fieldValues[f.name]} placeholder={f.defaultStr} list={`${f.name}-suggestions`} />
        {#if f.examples.length > 0}
          <datalist id={`${f.name}-suggestions`}>
            {#each f.examples as ex (ex.value)}
              <option value={ex.value}>{ex.help}</option>
            {/each}
          </datalist>
        {/if}
      {/if}

      <div class="actions">
        <button type="button" onclick={previousField}>{$t("newRemote.back")}</button>
        <button type="button" onclick={nextField} disabled={!canProceedFromCurrentField()}>
          {$t("advancedRemote.next")}
        </button>
      </div>
    </div>
  {:else if phase === "advancedPrompt"}
    <div class="step">
      <h2>{$t("advancedRemote.advancedPromptTitle")}</h2>
      <p class="field-help">{$t("advancedRemote.advancedPromptHint", { values: { count: advancedFields.length } })}</p>
      {#if errorMessage}<p class="error">✗ {errorMessage}</p>{/if}
      <div class="actions">
        <button type="button" onclick={() => (phase = "name")}>{$t("newRemote.back")}</button>
        <div class="actions-right">
          <button type="button" onclick={() => answerAdvancedPrompt(false)}>{$t("advancedRemote.skipAdvanced")}</button>
          <button type="button" onclick={() => answerAdvancedPrompt(true)}>{$t("advancedRemote.showAdvancedShort")}</button>
        </div>
      </div>
    </div>
  {:else if phase === "done"}
    <p class="ok">✓ {$t("newRemote.remoteCreated", { values: { name } })}</p>
    <a href="/">{$t("newRemote.backToHome")}</a>
  {/if}
</main>

<style>
.container {
  margin: 0 auto;
  max-width: 34em;
  padding: 4vh 1.5em;
}

.subtitle {
  color: var(--text-muted);
  margin-top: 1em;
}

.back-link {
  display: inline-block;
  color: var(--accent);
  margin-bottom: 0.9em;
}

.search-box {
  width: 100%;
  margin-bottom: 0.9em;
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

.provider-list {
  display: flex;
  flex-direction: column;
  gap: 0.6em;
  max-height: 65vh;
  overflow-y: auto;
  padding-right: 0.3em;
}

/* Una domanda alla volta: niente form lungo, ogni passo è una schermata a
   sé con titolo, aiuto e un solo campo. */
.step {
  display: flex;
  flex-direction: column;
  gap: 0.9em;
  margin-top: 1.5em;
}

.step h2 {
  margin: 0;
}

.step-progress {
  color: var(--text-muted);
  font-size: 0.85em;
  text-transform: uppercase;
  letter-spacing: 0.03em;
}

.required-marker {
  color: var(--error);
}

.field-help {
  color: var(--text-muted);
  font-size: 0.9em;
  white-space: pre-line;
  margin: 0;
}

.oauth-wait {
  padding: 0.9em 1.1em;
  border-radius: 8px;
  background-color: var(--accent-bg);
  margin-top: 1.5em;
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

.actions {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-top: 0.5em;
}

.actions-right {
  display: flex;
  gap: 0.6em;
}
</style>
