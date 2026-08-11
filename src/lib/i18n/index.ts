import { init, addMessages, locale, _ } from "svelte-i18n";
import { get } from "svelte/store";

export { _ as t };

export interface LocaleInfo {
  code: string;
  name: string;
}

const STORAGE_KEY = "rclone-easy-locale";
const FALLBACK = "en";

// Ogni file src/lib/i18n/locales/<codice>.json diventa automaticamente una
// lingua disponibile, senza toccare questo file: import.meta.glob scopre i
// file a build time, e il nome mostrato nel selettore viene ricavato dal
// codice via Intl.DisplayNames (l'endonimo, es. "italiano" per "it") invece
// di essere hard-codato — così un contributo di terzi si riduce ad
// aggiungere un JSON.
type LocaleMessages = { [key: string]: string | LocaleMessages };

const modules = import.meta.glob<{ default: LocaleMessages }>("./locales/*.json", { eager: true });

function displayName(code: string): string {
  try {
    const name = new Intl.DisplayNames([code], { type: "language" }).of(code);
    return name ? name.charAt(0).toUpperCase() + name.slice(1) : code;
  } catch {
    return code;
  }
}

export const locales: LocaleInfo[] = Object.entries(modules)
  .map(([path, mod]) => {
    const code = path.match(/([^/]+)\.json$/)![1];
    addMessages(code, mod.default);
    return { code, name: displayName(code) };
  })
  .sort((a, b) => a.code.localeCompare(b.code));

function detectDefault(): string {
  const codes = locales.map((l) => l.code);
  const candidates = typeof navigator !== "undefined" ? (navigator.languages ?? [navigator.language]) : [];
  for (const lang of candidates) {
    const short = lang.slice(0, 2).toLowerCase();
    if (codes.includes(short)) return short;
  }
  return codes.includes(FALLBACK) ? FALLBACK : (codes[0] ?? FALLBACK);
}

function readStored(): string | null {
  if (typeof localStorage === "undefined") return null;
  const stored = localStorage.getItem(STORAGE_KEY);
  return stored && locales.some((l) => l.code === stored) ? stored : null;
}

init({
  fallbackLocale: FALLBACK,
  initialLocale: readStored() ?? detectDefault(),
});

export function getLocale(): string {
  return get(locale) ?? FALLBACK;
}

export function setLocale(code: string) {
  locale.set(code);
  if (typeof localStorage !== "undefined") localStorage.setItem(STORAGE_KEY, code);
}
