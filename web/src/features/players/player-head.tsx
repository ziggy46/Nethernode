import { useState } from "react";

import { cn } from "@/lib/utils";

/**
 * A player's face from their skin (fetched and cached by NetherNode), or
 * an original pixel face when there's no skin to show (offline-mode
 * players, no network). Never shows Mojang's default skins.
 */
export function PlayerHead({
  uuid,
  name,
  size = 32,
  className,
}: {
  uuid: string;
  name: string;
  size?: number;
  className?: string;
}) {
  const [failed, setFailed] = useState(false);
  const style = { width: size, height: size };
  if (failed || !uuid) {
    return <FallbackHead style={style} className={className} />;
  }
  return (
    <img
      src={`/api/players/${uuid}/head`}
      alt=""
      aria-hidden
      width={size}
      height={size}
      loading="lazy"
      onError={() => setFailed(true)}
      className={cn("shrink-0 rounded-xs [image-rendering:pixelated]", className)}
      style={style}
      title={name}
    />
  );
}

function FallbackHead({ style, className }: { style: React.CSSProperties; className?: string }) {
  return (
    <svg
      viewBox="0 0 8 8"
      aria-hidden
      className={cn("shrink-0 rounded-xs", className)}
      style={style}
      shapeRendering="crispEdges"
    >
      <rect width="8" height="8" className="fill-surface-elevated" />
      <rect y="0" width="8" height="2" className="fill-border-strong" />
      <rect x="1" y="3" width="2" height="1" className="fill-text-muted" />
      <rect x="5" y="3" width="2" height="1" className="fill-text-muted" />
      <rect x="3" y="5" width="2" height="1" className="fill-border-strong" />
    </svg>
  );
}
