import { PageHeader } from "@/components/page";
import { useDocumentTitle } from "@/lib/use-document-title";

import { PlayersView } from "./players-view";

/** Everyone who has played on any server. */
export function PlayersPage() {
  useDocumentTitle("Players");
  return (
    <>
      <PageHeader
        eyebrow="Community"
        title="Players"
        description="Everyone who has joined your servers. Open a server's Players tab to manage them there."
      />
      <PlayersView />
    </>
  );
}
