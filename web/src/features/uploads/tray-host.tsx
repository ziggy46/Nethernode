import { lazy, Suspense, useEffect, useSyncExternalStore } from "react";

import { getUploads, subscribe, uploading } from "./store";

const UploadTray = lazy(() => import("./upload-tray"));

/** Shows the upload tray once there's something in it, on every page. */
export function UploadTrayHost() {
  const items = useSyncExternalStore(subscribe, getUploads);

  // Leaving the page stops uploads (choosing the same file again resumes
  // them), so ask first.
  useEffect(() => {
    const warn = (e: BeforeUnloadEvent) => {
      if (uploading()) e.preventDefault();
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, []);

  if (items.length === 0) return null;
  return (
    <Suspense fallback={null}>
      <UploadTray items={items} />
    </Suspense>
  );
}
