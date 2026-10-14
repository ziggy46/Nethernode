/**
 * Uploads in progress, shared by every page so they keep going while you
 * navigate. The tus client itself loads on the first upload.
 */
import { csrfToken } from "@/lib/api";

import type { TusHandle } from "./tus";

export type Conflict = "rename" | "overwrite" | "skip";

export type UploadStatus =
  | "queued"
  | "uploading"
  | "paused"
  | "finishing"
  | "done"
  | "skipped"
  | "failed"
  | "cancelled";

export interface UploadItem {
  key: string;
  serverId: string;
  serverName: string;
  dir: string;
  name: string;
  size: number;
  sent: number;
  status: UploadStatus;
  /** Bytes per second, smoothed. */
  speed: number;
  /** Outcome or problem, for display. */
  message?: string;
  /** Where the file ended up. */
  path?: string;
}

export interface UploadTarget {
  serverId: string;
  serverName: string;
  dir: string;
}

export interface UploadOptions {
  conflict: Conflict;
  extract: boolean;
  /** Continue this server-side upload (`/api/uploads/<id>`). */
  resumeUrl?: string;
}

interface Entry {
  item: UploadItem;
  file: File;
  options: UploadOptions;
  handle?: TusHandle;
  uploadId?: string;
  samples: [number, number][];
  fallback?: ReturnType<typeof setTimeout>;
}

interface Result {
  path: string;
  message: string;
  skipped: boolean;
  failed: boolean;
}

const PARALLEL = 3;
/** How long to wait for `upload_finished` before asking the server. */
const FINISH_CHECK_MS = 20_000;

let entries: Entry[] = [];
let snapshot: UploadItem[] = [];
const listeners = new Set<() => void>();
/** Results that arrived before this tab learned the upload's ID. */
const early = new Map<string, Result>();
let emitTimer: ReturnType<typeof setTimeout> | undefined;
let nextKey = 0;

function emitNow() {
  emitTimer = undefined;
  snapshot = entries.map((e) => e.item);
  for (const listener of listeners) listener();
}

/** Coalesces progress updates (they arrive many times a second). */
function emitSoon() {
  emitTimer ??= setTimeout(emitNow, 200);
}

function set(entry: Entry, patch: Partial<UploadItem>, now = true) {
  entry.item = { ...entry.item, ...patch };
  if (now) {
    if (emitTimer) clearTimeout(emitTimer);
    emitNow();
  } else {
    emitSoon();
  }
}

export function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getUploads(): UploadItem[] {
  return snapshot;
}

/** Whether anything is still sending (for the leave-page warning). */
export function uploading(): boolean {
  return entries.some((e) => ["queued", "uploading"].includes(e.item.status));
}

let tus: Promise<typeof import("./tus")> | undefined;
/** Loads the tus client once. */
const loadTus = () => {
  tus ??= import("./tus").catch((error: unknown) => {
    // Let the next upload try again (say, after a network blip).
    tus = undefined;
    throw error;
  });
  return tus;
};

const active = () => entries.filter((e) => e.item.status === "uploading").length;

function pump() {
  for (const entry of entries) {
    if (active() >= PARALLEL) return;
    if (entry.item.status === "queued") void start(entry);
  }
}

async function start(entry: Entry) {
  set(entry, { status: "uploading", message: undefined });
  entry.samples = [[performance.now(), entry.item.sent]];
  try {
    const [{ startUpload }, csrf] = await Promise.all([loadTus(), csrfToken()]);
    if (entry.item.status !== "uploading") return;
    entry.handle = await startUpload(
      entry.file,
      {
        serverId: entry.item.serverId,
        dir: entry.item.dir,
        conflict: entry.options.conflict,
        extract: entry.options.extract,
        resumeUrl: entry.options.resumeUrl,
        csrf,
      },
      {
        onProgress: (sent) => progress(entry, sent),
        onSuccess: (uploadId) => arrived(entry, uploadId),
        onError: (message) => {
          set(entry, { status: "failed", message, speed: 0 });
          pump();
        },
      },
    );
    // Paused or cancelled while the upload was starting.
    if (entry.item.status !== "uploading") {
      void entry.handle.abort(entry.item.status === "cancelled").catch(() => undefined);
      entry.handle = undefined;
    }
  } catch (error) {
    set(entry, { status: "failed", message: error instanceof Error ? error.message : "The upload failed." });
    pump();
  }
}

function progress(entry: Entry, sent: number) {
  if (entry.item.status !== "uploading") return;
  const now = performance.now();
  entry.samples.push([now, sent]);
  // Speed over the last five seconds.
  while (entry.samples.length > 2 && now - (entry.samples[0]?.[0] ?? now) > 5000) entry.samples.shift();
  const [t0, s0] = entry.samples[0] ?? [now, sent];
  const speed = now - t0 > 500 ? ((sent - s0) / (now - t0)) * 1000 : entry.item.speed;
  set(entry, { sent, speed }, false);
}

function arrived(entry: Entry, uploadId: string) {
  entry.uploadId = uploadId;
  entry.handle = undefined;
  set(entry, { status: "finishing", sent: entry.item.size, speed: 0 });
  pump();
  const result = early.get(uploadId);
  if (result) {
    early.delete(uploadId);
    settle(entry, result);
    return;
  }
  // If the event was missed (a dropped event stream), ask the server.
  entry.fallback = setTimeout(() => void checkFinished(entry), FINISH_CHECK_MS);
}

async function checkFinished(entry: Entry) {
  if (entry.item.status !== "finishing") return;
  try {
    const response = await fetch("/api/uploads", { credentials: "same-origin" });
    const unfinished = (await response.json()) as { id: string }[];
    if (unfinished.some((u) => u.id === entry.uploadId)) {
      entry.fallback = setTimeout(() => void checkFinished(entry), FINISH_CHECK_MS);
      return;
    }
    set(entry, { status: "done", message: "Uploaded. Refresh the folder to see it." });
  } catch {
    entry.fallback = setTimeout(() => void checkFinished(entry), FINISH_CHECK_MS);
  }
}

function settle(entry: Entry, result: Result) {
  if (entry.fallback) clearTimeout(entry.fallback);
  set(entry, {
    status: result.failed ? "failed" : result.skipped ? "skipped" : "done",
    message: result.message,
    path: result.path || undefined,
  });
}

/** The server finished placing an upload (`upload_finished`). */
export function uploadFinished(uploadId: string, result: Result) {
  const entry = entries.find((e) => e.uploadId === uploadId);
  if (entry) {
    settle(entry, result);
    return;
  }
  early.set(uploadId, result);
  if (early.size > 100) {
    const oldest = early.keys().next().value;
    if (oldest !== undefined) early.delete(oldest);
  }
}

/** Queues `files` for upload into `target`. */
export function enqueue(target: UploadTarget, files: File[], options: UploadOptions) {
  for (const file of files) {
    entries.push({
      item: {
        key: String(nextKey++),
        ...target,
        name: file.name,
        size: file.size,
        sent: 0,
        status: "queued",
        speed: 0,
      },
      file,
      options,
      samples: [],
    });
  }
  emitNow();
  pump();
}

const find = (key: string) => entries.find((e) => e.item.key === key);

export function pause(key: string) {
  const entry = find(key);
  if (!entry || !["queued", "uploading"].includes(entry.item.status)) return;
  void entry.handle?.abort(false);
  entry.handle = undefined;
  set(entry, { status: "paused", speed: 0 });
  pump();
}

export function resume(key: string) {
  const entry = find(key);
  if (!entry || !["paused", "failed"].includes(entry.item.status) || entry.uploadId) return;
  set(entry, { status: "queued", message: undefined });
  pump();
}

export function cancel(key: string) {
  const entry = find(key);
  if (!entry || !["queued", "uploading", "paused", "failed"].includes(entry.item.status)) return;
  void entry.handle?.abort(true).catch(() => undefined);
  entry.handle = undefined;
  set(entry, { status: "cancelled", speed: 0, message: undefined });
  pump();
}

const ENDED: UploadStatus[] = ["done", "skipped", "failed", "cancelled"];

export function dismiss(key: string) {
  entries = entries.filter((e) => e.item.key !== key || !ENDED.includes(e.item.status));
  emitNow();
}

export function clearEnded() {
  entries = entries.filter((e) => !ENDED.includes(e.item.status));
  emitNow();
}
