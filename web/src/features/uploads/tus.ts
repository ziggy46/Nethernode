/**
 * The tus client, loaded on the first upload (it isn't needed to show the
 * panel). Chunks stay well under Cloudflare's 100 MB request limit, and an
 * interrupted upload resumes from the last chunk that reached the server.
 */
import { type DetailedError, type PreviousUpload, Upload, type UrlStorage } from "tus-js-client";

import { CSRF_HEADER_NAME } from "@/lib/api";

const CHUNK_SIZE = 8 * 1024 * 1024;
const RETRY_DELAYS = [0, 1000, 3000, 5000, 10000];
const PREFIX = "nn-upload::";

/**
 * Remembers unfinished uploads in localStorage so choosing the same file
 * again resumes it. Storage can be missing or full (private windows); then
 * uploads still work, they just can't resume after a reload.
 */
class SafeStorage implements UrlStorage {
  private entries(): [string, PreviousUpload][] {
    const out: [string, PreviousUpload][] = [];
    try {
      for (let i = 0; i < localStorage.length; i++) {
        const key = localStorage.key(i);
        if (!key?.startsWith(PREFIX)) continue;
        const raw = localStorage.getItem(key);
        if (raw) out.push([key, { ...(JSON.parse(raw) as PreviousUpload), urlStorageKey: key }]);
      }
    } catch {
      // Unavailable or corrupt: nothing to resume.
    }
    return out;
  }

  async findAllUploads() {
    return this.entries().map(([, upload]) => upload);
  }

  async findUploadsByFingerprint(fingerprint: string) {
    return this.entries()
      .filter(([key]) => key.startsWith(`${PREFIX}${fingerprint}::`))
      .map(([, upload]) => upload);
  }

  async removeUpload(key: string) {
    try {
      localStorage.removeItem(key);
    } catch {
      // Nothing to clean up.
    }
  }

  async addUpload(fingerprint: string, upload: PreviousUpload) {
    const key = `${PREFIX}${fingerprint}::${Math.round(Math.random() * 1e12)}`;
    try {
      localStorage.setItem(key, JSON.stringify(upload));
    } catch {
      // Can't remember it; the upload continues regardless.
    }
    return key;
  }
}

const storage = new SafeStorage();

export interface TusTarget {
  serverId: string;
  dir: string;
  conflict: "rename" | "overwrite" | "skip";
  extract: boolean;
  /** Continue this server-side upload instead of looking one up. */
  resumeUrl?: string;
  csrf?: string;
}

export interface TusCallbacks {
  onProgress(sent: number, total: number): void;
  /** Every byte arrived; `uploadId` identifies its `upload_finished` event. */
  onSuccess(uploadId: string): void;
  onError(message: string): void;
}

export interface TusHandle {
  /** Stops sending. With `cancel`, also discards what the server has. */
  abort(cancel: boolean): Promise<void>;
}

function messageOf(error: Error | DetailedError): string {
  const response = "originalResponse" in error ? error.originalResponse : null;
  const body = response?.getBody()?.trim();
  const status = response?.getStatus();
  if (body && status && status >= 400 && status < 600 && body.length < 300) return body;
  if (!response) return "Can't reach NetherNode. The upload will resume when you try again.";
  return error.message.split("\n")[0] ?? "The upload failed.";
}

/** Starts (or resumes) uploading `file`. */
export async function startUpload(
  file: File,
  target: TusTarget,
  callbacks: TusCallbacks,
): Promise<TusHandle> {
  const upload: Upload = new Upload(file, {
    endpoint: "/api/uploads",
    uploadUrl: target.resumeUrl ?? null,
    chunkSize: CHUNK_SIZE,
    retryDelays: RETRY_DELAYS,
    metadata: {
      filename: file.name,
      filetype: file.type,
      server: target.serverId,
      dir: target.dir,
      conflict: target.conflict,
      ...(target.extract ? { extract: "1" } : {}),
    },
    headers: target.csrf ? { [CSRF_HEADER_NAME]: target.csrf } : {},
    // The same file going to another folder is a different upload.
    fingerprint: async (f) =>
      ["v1", target.serverId, target.dir, f.name, f.size, f.lastModified].map(encodeURIComponent).join("/"),
    urlStorage: storage,
    storeFingerprintForResuming: true,
    removeFingerprintOnSuccess: true,
    onProgress: (sent, total) => callbacks.onProgress(sent, total),
    onSuccess: () => callbacks.onSuccess(upload.url?.split("/").pop() ?? ""),
    onError: (error) => callbacks.onError(messageOf(error)),
    onShouldRetry: (error) => {
      const status = error.originalResponse?.getStatus() ?? 0;
      // Network trouble, server errors, and "busy, try again" are worth
      // retrying; anything else (no space, too large, signed out) isn't.
      return status === 0 || (status >= 500 && status !== 507) || status === 409 || status === 423;
    },
  });
  if (!target.resumeUrl) {
    const previous = await upload.findPreviousUploads();
    const latest = previous.sort((a, b) => b.creationTime.localeCompare(a.creationTime))[0];
    if (latest) upload.resumeFromPreviousUpload(latest);
  }
  upload.start();
  return { abort: (cancel) => upload.abort(cancel) };
}
