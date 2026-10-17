import type { QueryClient } from "@tanstack/react-query";
import { createRootRouteWithContext, Link, Outlet } from "@tanstack/react-router";
import { CircleAlert } from "lucide-react";
import { lazy, Suspense } from "react";

import { ConnectionError } from "@/components/connection-error";
import { EmptyState } from "@/components/page";
import { Button } from "@/components/ui/button";

const Toaster = lazy(() => import("@/components/toaster"));

export interface RouterContext {
  queryClient: QueryClient;
}

export const Route = createRootRouteWithContext<RouterContext>()({
  component: Root,
  errorComponent: ({ error }) => <ConnectionError error={error} />,
  notFoundComponent: NotFound,
});

function Root() {
  return (
    <>
      <Outlet />
      <Suspense>
        <Toaster />
      </Suspense>
    </>
  );
}

function NotFound() {
  return (
    <EmptyState
      icon={<CircleAlert />}
      title="Page not found"
      action={
        <Button asChild variant="primary">
          <Link to="/">Back to overview</Link>
        </Button>
      }
    >
      That page doesn't exist. It may have moved, or the link may be mistyped.
    </EmptyState>
  );
}
