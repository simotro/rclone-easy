<script lang="ts">
  import ImportRemotesPanel from "$lib/components/ImportRemotesPanel.svelte";
</script>

<main class="container">
  <a href="/" class="back-link">← Torna alla schermata principale</a>
  <h1>Importa remote esistenti</h1>
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
