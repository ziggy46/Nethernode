import { useId, useState } from "react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

import type { Conflict } from "./store";

/** The choices, worded for one file or several. */
function choices(one: boolean): [Conflict, string, string][] {
  return one
    ? [
        ["overwrite", "Replace it", "The uploaded file takes its place."],
        ["rename", "Keep both", "The upload gets a number added, like world (1).zip."],
        ["skip", "Skip it", "The existing file stays as it is."],
      ]
    : [
        ["overwrite", "Replace them", "The uploaded files take their place."],
        ["rename", "Keep both", "Uploads get a number added, like world (1).zip."],
        ["skip", "Skip them", "Only files with new names are uploaded."],
      ];
}

/**
 * Asked before uploading when names are already taken or there are zip
 * files to unpack.
 */
export function UploadOptionsDialog({
  files,
  taken,
  folder,
  onConfirm,
  onCancel,
}: {
  files: File[];
  /** Names already in the folder. */
  taken: string[];
  folder: string;
  onConfirm: (options: { conflict: Conflict; extract: boolean }) => void;
  onCancel: () => void;
}) {
  const ids = { extract: useId(), group: useId() };
  const [conflict, setConflict] = useState<Conflict>("overwrite");
  const [extract, setExtract] = useState(false);
  const zips = files.filter((f) => f.name.toLowerCase().endsWith(".zip"));
  const single = taken.length === 1 ? taken[0] : undefined;

  return (
    <Dialog open onOpenChange={(open) => !open && onCancel()}>
      <DialogContent className="w-[480px]">
        <DialogHeader>
          <DialogTitle>Upload {files.length === 1 ? files[0]?.name : `${files.length} files`}</DialogTitle>
          <DialogDescription>To {folder}.</DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            onConfirm({ conflict, extract });
          }}
        >
          {taken.length > 0 && (
            <fieldset className="mb-[17px]" aria-describedby={ids.group}>
              <legend className="mb-1 text-label text-text-secondary">
                {single
                  ? `${single} already exists here.`
                  : `${taken.length} of these names already exist here.`}
              </legend>
              <p
                id={ids.group}
                className="mb-2.5 truncate text-meta text-text-muted"
                title={taken.join(", ")}
              >
                {single ? "What should happen to it?" : taken.join(", ")}
              </p>
              <div className="flex flex-col gap-2">
                {choices(taken.length === 1).map(([value, label, hint]) => (
                  <label
                    key={value}
                    className="flex cursor-pointer items-start gap-2.5 rounded-md border border-divider px-3 py-2.5 has-checked:border-primary"
                  >
                    <input
                      type="radio"
                      name="conflict"
                      value={value}
                      checked={conflict === value}
                      onChange={() => setConflict(value)}
                      className="mt-0.5 accent-primary"
                    />
                    <span>
                      <span className="block text-label">{label}</span>
                      <span className="block text-meta text-text-muted">{hint}</span>
                    </span>
                  </label>
                ))}
              </div>
            </fieldset>
          )}
          {zips.length > 0 && (
            <div className="mb-[17px] flex items-start gap-2.5">
              <Checkbox
                id={ids.extract}
                checked={extract}
                onCheckedChange={(v) => setExtract(v === true)}
                className="mt-0.5"
              />
              <label htmlFor={ids.extract} className="text-label text-text-secondary">
                Extract {zips.length === 1 ? zips[0]?.name : `the ${zips.length} zip files`} after uploading
                <span className="block text-meta text-text-muted">
                  Into this folder. If any file in it already exists here, nothing is extracted.
                </span>
              </label>
            </div>
          )}
          <DialogFooter>
            <Button onClick={onCancel}>Cancel</Button>
            <Button type="submit" variant="primary">
              Upload
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
