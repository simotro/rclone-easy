<script lang="ts">
  import "$lib/shared-styles.css";
  import { initTheme } from "$lib/theme.svelte";
  import ThemeToggle from "$lib/components/ThemeToggle.svelte";

  let { children } = $props();

  initTheme();
</script>

<ThemeToggle />
{@render children()}
