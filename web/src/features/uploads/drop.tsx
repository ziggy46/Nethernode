import { Upload } from "lucide-react";
import { type DragEvent, useRef, useState } from "react";

/**
 * Drag-and-drop for files over a whole page area. Spread `handlers` on the
 * container (which should be `relative`) and render `<DropOverlay>` inside
 * it while `dragging`.
 */
export function useFileDrop(onFiles: (files: File[]) => void, enabled = true) {
  const [dragging, setDragging] = useState(false);
  // Enter/leave fire for every child crossed; count them.
  const depth = useRef(0);
  const hasFiles = (e: DragEvent) => enabled && e.dataTransfer.types.includes("Files");

  const handlers = {
    onDragEnter: (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth.current += 1;
      setDragging(true);
    },
    onDragOver: (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "copy";
    },
    onDragLeave: (e: DragEvent) => {
      if (!hasFiles(e)) return;
      depth.current = Math.max(0, depth.current - 1);
      if (depth.current === 0) setDragging(false);
    },
    onDrop: (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth.current = 0;
      setDragging(false);
      // Folders show up as zero-byte entries with no type; skip them.
      const files = Array.from(e.dataTransfer.items ?? [])
        .filter((item) => item.kind === "file" && !item.webkitGetAsEntry?.()?.isDirectory)
        .map((item) => item.getAsFile())
        .filter((f): f is File => f !== null);
      if (files.length > 0) onFiles(files);
    },
  };
  return { dragging, handlers };
}

export function DropOverlay({ label }: { label: string }) {
  return (
    <div className="pointer-events-none absolute inset-0 z-10 grid place-items-center rounded-[9px] border-2 border-primary border-dashed bg-overlay">
      <p className="flex items-center gap-2 px-4 text-center font-semibold text-label">
        <Upload className="size-4 shrink-0 text-primary" aria-hidden />
        {label}
      </p>
    </div>
  );
}
