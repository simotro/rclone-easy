export type ThemeChoice = "system" | "light" | "dark";

const STORAGE_KEY = "rclone-easy-theme";

function readStored(): ThemeChoice {
  if (typeof localStorage === "undefined") return "system";
  const stored = localStorage.getItem(STORAGE_KEY);
  return stored === "light" || stored === "dark" ? stored : "system";
}

// $state a livello di modulo: un solo store condiviso da ThemeToggle e da
// chiunque altro debba leggere/impostare il tema, senza bisogno di context
// Svelte (un solo layout, nessun caso reale di più istanze isolate).
let current = $state<ThemeChoice>(readStored());

function apply(choice: ThemeChoice) {
  if (typeof document === "undefined") return;
  if (choice === "system") document.documentElement.removeAttribute("data-theme");
  else document.documentElement.setAttribute("data-theme", choice);
}

export function initTheme() {
  apply(current);
}

export function getTheme(): ThemeChoice {
  return current;
}

export function setTheme(choice: ThemeChoice) {
  current = choice;
  apply(choice);
  if (typeof localStorage !== "undefined") {
    if (choice === "system") localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, choice);
  }
}
