// Tipi condivisi tra +page.svelte (che carica mount/job/bisync una sola
// volta per tutti i remote) e RemoteRow.svelte (che li filtra per il
// proprio remote) — centralizzati qui per non doverli duplicare nei due
// file, con il rischio che finiscano per divergere.

export type MountEvent = { action: string; success: boolean; message: string; whenUnix: number };
export type MountEntry = {
  name: string;
  remote: string;
  mountPoint: string;
  mounted: boolean;
  autoMount: boolean;
  history: MountEvent[];
};

export type RunEntry = { success: boolean; message: string; whenUnix: number };
export type SyncJob = {
  name: string;
  source: string;
  destination: string;
  autoIntervalMinutes: number | null;
  propagateDeletions: boolean;
  history: RunEntry[];
  isRunning: boolean;
};

export type BisyncRunEntry = {
  success: boolean;
  message: string;
  whenUnix: number;
  conflictPaths: string[];
  log: string;
  needsForce: boolean;
};
export type BisyncJob = {
  name: string;
  path1: string;
  path2: string;
  needsResync: boolean;
  autoIntervalMinutes: number | null;
  history: BisyncRunEntry[];
  isRunning: boolean;
};
