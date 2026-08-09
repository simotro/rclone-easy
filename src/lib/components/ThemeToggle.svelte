<script lang="ts">
  import { getTheme, setTheme, type ThemeChoice } from "$lib/theme.svelte";

  const CONTRAST_ICON = `<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2"/><path d="M12 3a9 9 0 0 1 0 18z" fill="currentColor"/></svg>`;
  const SUN_ICON = `<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="12" cy="12" r="4.2" stroke="currentColor" stroke-width="2"/><path d="M12 2v2.4M12 19.6V22M4.9 4.9l1.7 1.7M17.4 17.4l1.7 1.7M2 12h2.4M19.6 12H22M4.9 19.1l1.7-1.7M17.4 6.6l1.7-1.7" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`;
  const MOON_ICON = `<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M20 14.5A8.5 8.5 0 1 1 9.5 4a7 7 0 0 0 10.5 10.5z" fill="currentColor"/></svg>`;
  const CHEVRON_ICON = `<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M6 9l6 6 6-6" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`;

  const options: { value: ThemeChoice; label: string; icon: string }[] = [
    { value: "system", label: "Sistema", icon: CONTRAST_ICON },
    { value: "light", label: "Chiaro", icon: SUN_ICON },
    { value: "dark", label: "Scuro", icon: MOON_ICON },
  ];

  let open = $state(false);
  let container: HTMLDivElement | undefined = $state();

  const current = $derived(options.find((o) => o.value === getTheme()) ?? options[0]);

  function choose(value: ThemeChoice) {
    setTheme(value);
    open = false;
  }

  function onWindowClick(event: MouseEvent) {
    if (open && container && !container.contains(event.target as Node)) open = false;
  }

  function onWindowKeydown(event: KeyboardEvent) {
    if (open && event.key === "Escape") open = false;
  }
</script>

<svelte:window onclick={onWindowClick} onkeydown={onWindowKeydown} />

<div class="theme-toggle" bind:this={container}>
  <button type="button" class="trigger" aria-haspopup="true" aria-expanded={open} onclick={() => (open = !open)}>
    <span class="icon">{@html current.icon}</span>
    <span>Tema</span>
    <span class="chevron" class:open>{@html CHEVRON_ICON}</span>
  </button>

  {#if open}
    <div class="menu" role="menu" aria-label="Scegli il tema">
      {#each options as option (option.value)}
        <button type="button" class="item" class:active={option.value === current.value} role="menuitem" onclick={() => choose(option.value)}>
          <span class="icon">{@html option.icon}</span>
          <span>{option.label}</span>
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
.theme-toggle {
  position: fixed;
  top: 0.9em;
  right: 1.1em;
  z-index: 50;
}

.trigger {
  display: inline-flex;
  align-items: center;
  gap: 0.5em;
  padding: 0.5em 0.9em;
  border-radius: 0.6em;
  border: 1px solid transparent;
  background-color: var(--accent-bg);
  color: var(--accent);
  font-weight: 500;
  box-shadow: none;
}

.trigger:hover {
  border-color: var(--border-color-subtle);
}

.chevron {
  display: inline-flex;
  width: 0.9em;
  height: 0.9em;
  opacity: 0.7;
  transition: transform 0.15s ease;
}

.chevron.open {
  transform: rotate(180deg);
}

.chevron :global(svg) {
  width: 100%;
  height: 100%;
}

.menu {
  position: absolute;
  top: calc(100% + 0.4em);
  right: 0;
  display: flex;
  flex-direction: column;
  min-width: 11em;
  padding: 0.4em;
  border-radius: 0.7em;
  border: 1px solid var(--border-color-subtle);
  background-color: var(--bg-surface);
  box-shadow: var(--shadow-md);
}

.item {
  display: flex;
  align-items: center;
  gap: 0.7em;
  padding: 0.5em 0.7em;
  border-radius: 0.5em;
  border: none;
  box-shadow: none;
  background: transparent;
  color: var(--text-color);
  font-weight: 400;
  text-align: left;
}

.item:hover {
  background-color: var(--surface-tint);
}

.item.active {
  background-color: var(--accent-bg);
  color: var(--accent);
  font-weight: 500;
}

.icon {
  display: inline-flex;
  width: 1.2em;
  height: 1.2em;
  justify-content: center;
  opacity: 0.85;
}

.icon :global(svg) {
  width: 100%;
  height: 100%;
}
</style>
