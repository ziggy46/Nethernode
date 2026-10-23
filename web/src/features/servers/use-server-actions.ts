import { useMutation, useQueryClient } from "@tanstack/react-query";

import { errorMessage, serverClient } from "@/lib/api";
import { withToast } from "@/lib/toast";

type Action = "start" | "stop" | "restart" | "kill";
/** An action, or a stop or restart after warning players for some minutes. */
type Request = Action | { action: "stop" | "restart"; warnMinutes: number };

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
    mutationFn: async (request: Request) => {
      const { action, warnMinutes } =
        typeof request === "string" ? { action: request, warnMinutes: 0 } : request;
      switch (action) {
        case "start":
          return serverClient.startServer({ id });
        case "stop":
          return serverClient.stopServer({ id, warnMinutes });
        case "restart":
          return serverClient.restartServer({ id, warnMinutes });
        case "kill":
          return serverClient.killServer({ id });
      }
    },
    onSuccess: (_, request) => {
      const message =
        typeof request === "string" || request.warnMinutes === 0
          ? `${VERBS[typeof request === "string" ? request : request.action]} ${name}…`
          : `Warning players on ${name}: it ${request.action === "stop" ? "stops" : "restarts"} in ${request.warnMinutes} minute${request.warnMinutes === 1 ? "" : "s"}.`;
      withToast((toast) => toast(message));
      void queryClient.invalidateQueries({ queryKey: ["servers"] });
    },
    onError: (error, request) => {
      const action = typeof request === "string" ? request : request.action;
      const message = `Couldn't ${action} ${name}. ${errorMessage(error)}`;
      withToast((toast) => toast.error(message, { duration: Number.POSITIVE_INFINITY }));
    },
  });
}
