import { useCallback, useEffect, useRef, useState } from "react";

/** Connection state shown in the console header. */
export type SocketState = "connecting" | "live" | "reconnecting";

/** Frames from `/ws/servers/:id/console`. */
export type ConsoleFrame =
  | { type: "backlog" | "lines"; lines: string[] }
  | { type: "skipped"; count: number }
  | { type: "reply"; command: string; output: string }
  | { type: "error"; command: string; message: string };

const BACKOFF_MS = [1000, 2000, 5000, 10_000];

/**
 * Keeps a console WebSocket open while mounted, reconnecting with backoff
 * (NetherNode restarting, a proxy dropping the connection). Frames go to
 * `onFrame` through a ref, so the terminal writes without React re-rendering
 * per line. Each connection starts with a fresh backlog. Callers check the
 * user's role first; the server refuses the handshake otherwise.
 */
export function useConsoleSocket(serverId: string, onFrame: (frame: ConsoleFrame) => void) {
  const [state, setState] = useState<SocketState>("connecting");
  const socket = useRef<WebSocket | null>(null);
  const handler = useRef(onFrame);
  handler.current = onFrame;

  useEffect(() => {
    let closed = false;
    let attempt = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const connect = () => {
      const scheme = window.location.protocol === "https:" ? "wss" : "ws";
      const ws = new WebSocket(`${scheme}://${window.location.host}/ws/servers/${serverId}/console`);
      socket.current = ws;
      ws.onopen = () => {
        attempt = 0;
        setState("live");
      };
      ws.onmessage = (event) => {
        try {
          handler.current(JSON.parse(String(event.data)) as ConsoleFrame);
        } catch {
          // Ignore frames we can't parse.
        }
      };
      ws.onclose = () => {
        // A replaced socket's late close must not clear its successor.
        if (socket.current === ws) socket.current = null;
        if (closed) return;
        setState("reconnecting");
        const delay = BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)];
        attempt += 1;
        timer = setTimeout(connect, delay);
      };
    };
    setState("connecting");
    connect();
    return () => {
      closed = true;
      clearTimeout(timer);
      socket.current?.close();
      socket.current = null;
    };
  }, [serverId]);

  const send = useCallback((command: string): boolean => {
    const ws = socket.current;
    if (!ws || ws.readyState !== WebSocket.OPEN) return false;
    ws.send(JSON.stringify({ type: "command", command }));
    return true;
  }, []);

  return { state, send };
}
