import { CSRF_HEADER_NAME, csrfToken } from "@/lib/api";

import { joinPath } from "./paths";

/** Largest file the simple upload accepts (matches the server). */
export const UPLOAD_LIMIT = 25 * 1024 * 1024;

export class UploadError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }

  /** The file already exists (retry with `overwrite`). */
  get exists() {
    return this.status === 409;
  }
}

/**
 * Uploads `file` into folder `dir`. Uses XMLHttpRequest because `fetch`
 * can't report upload progress.
 */
export async function uploadFile(
  serverId: string,
  dir: string,
  file: File,
  overwrite: boolean,
  onProgress?: (fraction: number) => void,
): Promise<void> {
  if (file.size > UPLOAD_LIMIT) {
    throw new UploadError(
      `${file.name} is over 25 MB. Larger uploads arrive with resumable uploads in a later update.`,
      413,
    );
  }
  const token = await csrfToken();
  const params = new URLSearchParams({ server: serverId, path: joinPath(dir, file.name) });
  if (overwrite) params.set("overwrite", "true");
  await new Promise<void>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", `/api/files/upload?${params}`);
    if (token) xhr.setRequestHeader(CSRF_HEADER_NAME, token);
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable) onProgress?.(e.loaded / e.total);
    };
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        resolve();
        return;
      }
      let message = `Uploading ${file.name} failed (${xhr.status}).`;
      try {
        const body = JSON.parse(xhr.responseText) as { message?: string };
        if (body.message) message = body.message;
      } catch {
        if (xhr.status === 413) message = `${file.name} is over 25 MB.`;
      }
      reject(new UploadError(message, xhr.status));
    };
    xhr.onerror = () => reject(new UploadError(`Uploading ${file.name} failed: can't reach NetherNode.`, 0));
    xhr.send(file);
  });
}
