/**
 * Toasts for background file jobs (extract, compress) started from this tab.
 * Other admins' jobs only refresh file lists. A quick job can finish before
 * the request that started it returns, so finished jobs are remembered
 * briefly and shown when the starter catches up.
 */
import { toast } from "sonner";

interface Result {
  message: string;
  failed: boolean;
}

const titles = new Map<string, string>();
const finished = new Map<string, Result>();
const REMEMBER = 100;

function show(jobId: string, result: Result) {
  if (result.failed) toast.error(result.message, { id: jobId, duration: Number.POSITIVE_INFINITY });
  else toast.success(result.message, { id: jobId });
}

/** A job was started here; show its progress under `title`. */
export function trackJob(jobId: string, title: string) {
  const done = finished.get(jobId);
  if (done) {
    finished.delete(jobId);
    show(jobId, done);
    return;
  }
  titles.set(jobId, title);
  toast.loading(title, { id: jobId });
}

export function jobProgress(jobId: string, percent: number) {
  const title = titles.get(jobId);
  if (title) toast.loading(`${title} ${percent}%`, { id: jobId });
}

export function jobFinished(jobId: string, result: Result) {
  if (titles.delete(jobId)) {
    show(jobId, result);
    return;
  }
  finished.set(jobId, result);
  if (finished.size > REMEMBER) {
    const oldest = finished.keys().next().value;
    if (oldest !== undefined) finished.delete(oldest);
  }
}
