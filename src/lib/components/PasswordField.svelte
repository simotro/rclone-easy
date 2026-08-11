<script lang="ts">
  import Icon from "./Icon.svelte";
  import { t } from "$lib/i18n";

  let {
    value = $bindable(""),
    label,
    placeholder,
    autofocus = false,
    disabled = false,
  }: {
    value: string;
    label: string;
    placeholder?: string;
    autofocus?: boolean;
    disabled?: boolean;
  } = $props();

  let visible = $state(false);
</script>

<label class="password-field">
  {label}
  <div class="password-input-row">
    <input type={visible ? "text" : "password"} bind:value {placeholder} {autofocus} {disabled} />
    <button
      type="button"
      class="reveal-toggle"
      onclick={() => (visible = !visible)}
      disabled={disabled}
      aria-label={visible ? $t("passwordField.hide") : $t("passwordField.show")}
      title={visible ? $t("passwordField.hide") : $t("passwordField.show")}
    >
      <Icon kind={visible ? "eye-off" : "eye"} />
    </button>
  </div>
</label>

<style>
.password-field {
  display: flex;
  flex-direction: column;
  gap: 0.3em;
  font-weight: 500;
}

.password-input-row {
  display: flex;
  gap: 0.4em;
  align-items: center;
}

.password-input-row input {
  flex: 1;
  min-width: 0;
}

.reveal-toggle {
  background: none;
  border: 1px solid var(--border-color);
  box-shadow: none;
  padding: 0.5em;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
}

.reveal-toggle:hover {
  color: var(--text-color);
}
</style>
