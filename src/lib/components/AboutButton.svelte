<script lang="ts">
  import { getVersion } from "@tauri-apps/api/app";
  import { invoke } from "@tauri-apps/api/core";
  import Modal from "./Modal.svelte";
  import { t } from "$lib/i18n";

  const REPO_URL = "https://github.com/simotro/rclone-easy";

  const HELP_ICON = `<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2"/><path d="M9.1 9a3 3 0 1 1 5.82 1c-.58 1.45-2.92 2-2.92 4" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/><line x1="12" y1="17" x2="12.01" y2="17" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`;

  let open = $state(false);
  let version = $state("");

  $effect(() => {
    if (open && version === "") getVersion().then((v) => (version = v));
  });

  async function openRepo() {
    await invoke("open_url_in_browser", { url: REPO_URL });
  }
</script>

<button type="button" class="about-button" title={$t("about.title")} aria-label={$t("about.title")} onclick={() => (open = true)}>
  {@html HELP_ICON}
</button>

<Modal bind:open title={$t("about.title")}>
  <div class="about-content">
    <img src="/app-icon.png" alt="" class="app-icon" />
    <h2>Rclone Easy</h2>
    <dl>
      <dt>{$t("about.version")}</dt>
      <dd>{version || "…"}</dd>
      <dt>{$t("about.license")}</dt>
      <dd>MIT</dd>
      <dt>{$t("about.author")}</dt>
      <dd>Simone Tromboni</dd>
    </dl>
    <button type="button" class="link-button" onclick={openRepo}>{$t("about.repoLink")}</button>
  </div>
</Modal>

<style>
.about-button {
  background: none;
  border: 1px solid var(--border-color);
  box-shadow: none;
  padding: 0.5em;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
}

.about-button:hover {
  color: var(--text-color);
}

.about-button :global(svg) {
  width: 1.3em;
  height: 1.3em;
  display: block;
}

.about-content {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.6em;
  text-align: center;
}

.app-icon {
  width: 3.5em;
  height: 3.5em;
}

.about-content h2 {
  margin: 0;
}

dl {
  display: grid;
  grid-template-columns: auto auto;
  gap: 0.3em 0.8em;
  margin: 0.4em 0;
}

dt {
  color: var(--text-muted);
  text-align: right;
}

dd {
  margin: 0;
  text-align: left;
  font-weight: 500;
}

.link-button {
  background: none;
  border: none;
  box-shadow: none;
  padding: 0;
  color: var(--accent);
  cursor: pointer;
  font-size: 0.9em;
}
</style>
