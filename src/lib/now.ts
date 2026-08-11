import { readable, get } from "svelte/store";
import { t } from "$lib/i18n";

/// Orologio condiviso, aggiornato una volta al secondo — usato dai
/// contatori "prossima sincronizzazione tra..." in più componenti
/// contemporaneamente, un solo timer invece di uno per componente.
export const now = readable(Date.now(), (set) => {
  const id = setInterval(() => set(Date.now()), 1000);
  return () => clearInterval(id);
});

export function formatCountdown(targetMs: number, nowMs: number): string {
  const translate = get(t);
  const remaining = targetMs - nowMs;
  if (remaining <= 0) return translate("now.momentarily");
  const totalSeconds = Math.floor(remaining / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return translate("now.countdown", { values: { time: `${pad(hours)}:${pad(minutes)}:${pad(seconds)}` } });
}

/// `null` se il job non ha una prossima esecuzione automatica prevista
/// (nessun intervallo configurato).
export function nextRunAtMs(lastRunWhenUnix: number | null, autoIntervalMinutes: number | null): number | null {
  if (autoIntervalMinutes === null) return null;
  const lastRunMs = (lastRunWhenUnix ?? 0) * 1000;
  return lastRunMs + autoIntervalMinutes * 60 * 1000;
}
