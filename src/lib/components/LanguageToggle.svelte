<script lang="ts">
  import { locale } from "svelte-i18n";
  import { setLocale, locales, t } from "$lib/i18n";

  const GLOBE_ICON = `<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18 14 14 0 0 1 0-18z" stroke="currentColor" stroke-width="2"/></svg>`;
  const CHEVRON_ICON = `<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M6 9l6 6 6-6" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`;

  let open = $state(false);
  let container: HTMLDivElement | undefined = $state();

  const current = $derived(locales.find((l) => l.code === $locale) ?? locales[0]);

  function choose(code: string) {
    setLocale(code);
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

<div class="language-toggle" bind:this={container}>
  <button type="button" class="trigger" aria-haspopup="true" aria-expanded={open} onclick={() => (open = !open)}>
    <span class="icon">{@html GLOBE_ICON}</span>
    <span>{current?.name}</span>
    <span class="chevron" class:open>{@html CHEVRON_ICON}</span>
  </button>

  {#if open}
    <div class="menu" role="menu" aria-label={$t("language.label")}>
      {#each locales as option (option.code)}
        <button type="button" class="item" class:active={option.code === current?.code} role="menuitem" onclick={() => choose(option.code)}>
          <span>{option.name}</span>
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
.language-toggle {
  position: relative;
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
  min-width: 9em;
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
