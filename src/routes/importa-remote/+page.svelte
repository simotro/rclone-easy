<script lang="ts">
  import ImportRemotesPanel from "$lib/components/ImportRemotesPanel.svelte";
  import { t } from "$lib/i18n";
</script>

<main class="container">
  <a href="/" class="back-link">← {$t("newRemote.backToHome")}</a>
  <h1>{$t("home.importExistingRemotes")}</h1>
  <ImportRemotesPanel />
</main>

<style>
.container {
  margin: 0 auto;
  max-width: 40em;
  padding: 4vh 1.5em;
}

.back-link {
  display: inline-block;
  margin-bottom: 1em;
}
</style>
