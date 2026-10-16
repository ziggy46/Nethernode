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

/** How long a failure stays up (it can be closed sooner). */
const ERROR_MS = 15_000;
/** A job not heard from in this long has its progress toast taken down. */
const STALE_MS = 30 * 60_000;
const stale = new Map<string, ReturnType<typeof setTimeout>>();

function show(jobId: string, result: Result) {
  clearTimeout(stale.get(jobId));
  stale.delete(jobId);
  if (result.failed) toast.error(result.message, { id: jobId, duration: ERROR_MS });
  else toast.success(result.message, { id: jobId });
}

/** Takes the toast down if the job goes quiet (its finish event was missed). */
function watch(jobId: string) {
  clearTimeout(stale.get(jobId));
  stale.set(
    jobId,
    setTimeout(() => {
      stale.delete(jobId);
      if (titles.delete(jobId)) toast.dismiss(jobId);
    }, STALE_MS),
  );
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
  watch(jobId);
}

export function jobProgress(jobId: string, percent: number) {
  const title = titles.get(jobId);
  if (!title) return;
  toast.loading(`${title} ${percent}%`, { id: jobId });
  watch(jobId);
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
