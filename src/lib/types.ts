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

export type DryRunReport = {
  sourceTotalFiles: number;
  destinationTotalFiles: number;
  wouldTransfer: number;
  wouldDelete: number;
  whenUnix: number;
};

export type TransferEvent = { name: string; what: string; error: string };
export type RunEntry = { success: boolean; message: string; whenUnix: number; transfers: TransferEvent[] };
export type SyncJob = {
  name: string;
  source: string;
  destination: string;
  autoIntervalMinutes: number | null;
  propagateDeletions: boolean;
  history: RunEntry[];
  lastDryRun: DryRunReport | null;
  isRunning: boolean;
  isDryRunning: boolean;
};

export type BisyncRunEntry = {
  success: boolean;
  message: string;
  whenUnix: number;
  conflictPaths: string[];
  duplicateNames: string[];
  log: string;
  needsForce: boolean;
  autoResynced: boolean;
  resyncReason: string;
};

export type BisyncProgress = {
  phase: "waitingLock" | "preparing" | "comparing" | "applying" | "finishing";
  detail: string | null;
  queuedTotal: number;
  transferring: { name: string; size: number; bytes: number; direction: string }[];
};

export type ActivityKind = "copied" | "deleted" | "moved" | "conflict" | "warning" | "error" | "mounted" | "unmounted";

export type ActivityEvent = {
  whenUnix: number;
  remote: string;
  service: "backup" | "bisync" | "mount";
  job: string;
  kind: ActivityKind;
  direction: string;
  path: string;
  size: number | null;
  message: string;
};

export type ActivityPage = {
  events: ActivityEvent[];
  total: number;
  remotes: string[];
};

export type BisyncDryRunReport = {
  path1TotalFiles: number;
  path2TotalFiles: number;
  wouldTransfer: number;
  wouldDelete: number;
  log: string;
  whenUnix: number;
};

export type TrashEntry = {
  trashPath: string;
  originalPath: string;
  whenUnix: number;
  size: number;
};

export type DuplicateObject = {
  id: string;
  size: number;
  modTime: string;
  fs: string;
};

export type ReviewEntry = {
  id: string;
  name: string;
  size: number;
  modTime: string;
  fs: string;
  reviewPath: string;
};

export type BisyncJob = {
  name: string;
  path1: string;
  path2: string;
  needsResync: boolean;
  autoIntervalMinutes: number | null;
  history: BisyncRunEntry[];
  lastDryRun: BisyncDryRunReport | null;
  isRunning: boolean;
  isDryRunning: boolean;
  progress: BisyncProgress | null;
};
