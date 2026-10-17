import { useMutation, useQueryClient } from "@tanstack/react-query";

import { errorMessage, serverClient } from "@/lib/api";
import { withToast } from "@/lib/toast";

type Action = "start" | "stop" | "restart" | "kill";

const VERBS: Record<Action, string> = {
  start: "Starting",
  stop: "Stopping",
  restart: "Restarting",
  kill: "Force-stopping",
};

/**
 * Lifecycle actions. The server confirms each step through status events,
 * so success here only means the request was accepted; the badge shows the
 * real state.
 */
export function useServerAction(id: string, name: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (action: Action) => {
      switch (action) {
        case "start":
          return serverClient.startServer({ id });
        case "stop":
          return serverClient.stopServer({ id });
        case "restart":
          return serverClient.restartServer({ id });
        case "kill":
          return serverClient.killServer({ id });
      }
    },
    onSuccess: (_, action) => {
      withToast((toast) => toast(`${VERBS[action]} ${name}…`));
      void queryClient.invalidateQueries({ queryKey: ["servers"] });
    },
    onError: (error, action) => {
      const message = `Couldn't ${action} ${name}. ${errorMessage(error)}`;
      withToast((toast) => toast.error(message, { duration: Number.POSITIVE_INFINITY }));
    },
  });
}
