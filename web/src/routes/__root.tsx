import type { QueryClient } from "@tanstack/react-query";
import { createRootRouteWithContext, Link, Outlet } from "@tanstack/react-router";
import { CircleAlert, CircleCheck, Info, TriangleAlert } from "lucide-react";
import { Toaster } from "sonner";

import { ConnectionError } from "@/components/connection-error";
import { EmptyState } from "@/components/page";
import { Button } from "@/components/ui/button";

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
      <Toaster
        position="bottom-center"
        icons={{
          success: <CircleCheck className="size-[15px] text-success" />,
          error: <CircleAlert className="size-[15px] text-danger" />,
          warning: <TriangleAlert className="size-[15px] text-warning" />,
          info: <Info className="size-[15px] text-primary" />,
        }}
        toastOptions={{
          unstyled: true,
          classNames: {
            toast:
              "flex w-full items-center gap-[9px] rounded-[9px] border border-primary/40 bg-surface-elevated px-[18px] py-3 text-label text-text-primary shadow-overlay",
            error: "border-l-[3px] border-l-danger",
            success: "border-l-[3px] border-l-success",
            warning: "border-l-[3px] border-l-warning",
          },
        }}
      />
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
