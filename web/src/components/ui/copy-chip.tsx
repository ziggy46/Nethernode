import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";

import { withToast } from "@/lib/toast";
import { cn } from "@/lib/utils";

/** Text set apart in a chip that copies it when clicked (a server's address,
 * a name to type back). */
export function CopyChip({
  text,
  mono,
  className,
}: {
  text: string;
  /** Monospace, for addresses. */
  mono?: boolean;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      withToast((toast) => toast(text, { description: "Couldn't copy automatically; select and copy it." }));
    }
  };

  return (
    <button
      type="button"
      onClick={copy}
      title={copied ? "Copied" : "Copy"}
      aria-label={copied ? `Copied ${text}` : `Copy ${text}`}
      className={cn(
        "inline-flex max-w-full items-center gap-1.5 rounded-md border border-border bg-surface-elevated px-2 py-0.5 align-middle font-[550] text-text-primary transition-colors hover:border-border-strong focus-visible:outline-2 focus-visible:outline-primary focus-visible:outline-offset-2",
        mono && "font-mono font-normal",
        className,
      )}
    >
      {copied ? (
        <Check className="size-3.5 shrink-0 text-success" aria-hidden />
      ) : (
        <Copy className="size-3.5 shrink-0 text-primary" aria-hidden />
      )}
      <span className="truncate">{text}</span>
    </button>
  );
}
