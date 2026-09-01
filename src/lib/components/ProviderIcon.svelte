<script lang="ts">
  // Glifo colorato con le iniziali del servizio, non il logo ufficiale:
  // niente questioni di marchio da gestire, resta leggero (nessun asset
  // esterno da caricare, solo testo+colore) e coerente con lo stile
  // minimalista già usato in Icon.svelte per le altre icone dell'app.
  const PALETTE: Record<string, { bg: string; label: string }> = {
    s3: { bg: "#b45309", label: "S3" },
    b2: { bg: "#dc2626", label: "B2" },
    dropbox: { bg: "#0061fe", label: "DB" },
    drive: { bg: "#16a34a", label: "GD" },
    "drive-shared": { bg: "#16a34a", label: "SD" },
    mega: { bg: "#db2777", label: "MG" },
    nextcloud: { bg: "#0082c9", label: "NC" },
    onedrive: { bg: "#0078d4", label: "OD" },
    owncloud: { bg: "#1d2d44", label: "OC" },
    sftp: { bg: "#475569", label: "SF" },
    webdav: { bg: "#7c3aed", label: "WD" },
  };

  let { kind }: { kind: string } = $props();
  let entry = $derived(PALETTE[kind] ?? { bg: "#666666", label: "?" });
</script>

<span class="provider-icon" style:background-color={entry.bg} aria-hidden="true">{entry.label}</span>

<style>
.provider-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 2.3em;
  height: 2.3em;
  border-radius: 8px;
  color: #ffffff;
  font-size: 0.72em;
  font-weight: 700;
  line-height: 1;
  flex-shrink: 0;
}
</style>
