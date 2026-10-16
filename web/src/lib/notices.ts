import { queryOptions } from "@tanstack/react-query";

/** Something that needs attention, from `GET /api/notices`. */
export interface Notice {
  id: string;
  level: "danger" | "warning" | "info";
  title: string;
  detail: string;
  /** A panel path, or a full URL. */
  link?: string | null;
}

export const noticesQuery = queryOptions({
  queryKey: ["notices"],
  queryFn: async (): Promise<Notice[]> => {
    const response = await fetch("/api/notices", { credentials: "same-origin" });
    if (!response.ok) throw new Error(`notices: ${response.status}`);
    return ((await response.json()) as { notices: Notice[] }).notices;
  },
  refetchInterval: 60_000,
});

const SEEN = "nn.notices.seen";

/** Notice IDs this browser has already shown. */
export function seenNotices(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(SEEN) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}

/** Remembers these as seen (and forgets ones that went away). */
export function markNoticesSeen(notices: Notice[]) {
  try {
    localStorage.setItem(SEEN, JSON.stringify(notices.map((n) => n.id)));
  } catch {
    // Private windows and blocked storage: the badge just comes back.
  }
}
