/**
 * Toasts for code in the signed-in shell (the event stream, file jobs, sign
 * out). The toast library loads after the first paint instead of with the
 * shell, so calls made here wait until the toaster is on screen. Pages that
 * load on demand import `sonner` directly.
 */
import type { toast as Toast } from "sonner";

type Sonner = typeof Toast;

let mounted: (toast: Sonner) => void = () => {};
const ready = new Promise<Sonner>((resolve) => {
  mounted = resolve;
});

/** Called by the toaster once it's mounted. */
export function toasterReady(toast: Sonner) {
  mounted(toast);
}

/** Runs `show` with the toast API once the toaster is ready. */
export function withToast(show: (toast: Sonner) => void) {
  void ready.then(show);
}
