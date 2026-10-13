import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { RefreshCw, ServerCrash } from "lucide-react";

import { Logo } from "@/components/brand";
import { Button } from "@/components/ui/button";
import { errorMessage } from "@/lib/api";

/** Full-page state when the panel API can't be reached or a route fails. */
export function ConnectionError({ error }: { error: unknown }) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const retry = async () => {
    await queryClient.resetQueries();
    await router.invalidate();
  };
  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-4 px-4 text-center">
      <Logo decorative className="size-12 opacity-60" />
      <h1 className="flex items-center gap-2 font-semibold text-title">
        <ServerCrash className="size-5 text-danger" aria-hidden />
        NetherNode is unreachable
      </h1>
      <p role="alert" className="max-w-[48ch] text-label text-text-muted">
        {errorMessage(error)}
      </p>
      <Button variant="primary" onClick={() => void retry()}>
        <RefreshCw />
        Try again
      </Button>
    </main>
  );
}
