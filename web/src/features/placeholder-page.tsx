import type * as React from "react";

import { EmptyState, PageHeader } from "@/components/page";
import { useDocumentTitle } from "@/lib/use-document-title";

/**
 * A section whose backend isn't built yet. Keeps the reference's page
 * header and states plainly that the feature isn't available.
 */
export function PlaceholderPage({
  title,
  eyebrow,
  heading,
  description,
  icon,
  emptyTitle,
  emptyBody,
}: {
  title: string;
  eyebrow: string;
  heading: string;
  description: string;
  icon: React.ReactNode;
  emptyTitle: string;
  emptyBody: string;
}) {
  useDocumentTitle(title);
  return (
    <>
      <PageHeader eyebrow={eyebrow} title={heading} description={description} />
      <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
        <EmptyState icon={icon} title={emptyTitle}>
          {emptyBody}
        </EmptyState>
      </div>
    </>
  );
}
