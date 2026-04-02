import { startTransition, useEffect, useEffectEvent, useState } from "react";

import { isSessionUpdatedEvent } from "../lib/trace-ui";
import type { SessionSummary } from "../types";

const SESSIONS_URL = "http://localhost:7842/api/sessions";
const STREAM_URL = "ws://localhost:7842/api/stream";

export function useSessions() {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useEffectEvent(async () => {
    try {
      const response = await fetch(SESSIONS_URL, {
        cache: "no-store",
      });
      if (!response.ok) {
        throw new Error(`Failed to load sessions (${response.status})`);
      }

      const nextSessions = (await response.json()) as SessionSummary[];
      startTransition(() => {
        setSessions(nextSessions);
      });
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Failed to load sessions");
    } finally {
      setLoading(false);
    }
  });

  useEffect(() => {
    void refresh();

    const socket = new WebSocket(STREAM_URL);
    socket.addEventListener("message", (event) => {
      try {
        const payload = JSON.parse(event.data) as unknown;
        if (isSessionUpdatedEvent(payload)) {
          void refresh();
        }
      } catch {
        setError((current) => current ?? "Received an invalid stream event");
      }
    });
    socket.addEventListener("error", () => {
      setError((current) => current ?? "Live updates unavailable");
    });

    return () => {
      socket.close();
    };
  }, []);

  return {
    sessions,
    loading,
    error,
    refresh,
  };
}
