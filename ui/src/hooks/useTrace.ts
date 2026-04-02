import { startTransition, useEffect, useEffectEvent, useState } from "react";

import type { SessionTraceResponse } from "../types";

export function useTrace(sessionId: string | null, refreshKey = 0) {
  const [trace, setTrace] = useState<SessionTraceResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useEffectEvent(async (targetSessionId: string) => {
    setLoading(true);

    try {
      const response = await fetch(
        `http://localhost:7842/api/sessions/${encodeURIComponent(targetSessionId)}/trace`,
        {
          cache: "no-store",
        },
      );

      if (!response.ok) {
        throw new Error(`Failed to load trace (${response.status})`);
      }

      const nextTrace = (await response.json()) as SessionTraceResponse;
      startTransition(() => {
        setTrace(nextTrace);
      });
      setError(null);
    } catch (cause) {
      setTrace(null);
      setError(cause instanceof Error ? cause.message : "Failed to load trace");
    } finally {
      setLoading(false);
    }
  });

  useEffect(() => {
    if (!sessionId) {
      setTrace(null);
      setError(null);
      setLoading(false);
      return;
    }

    void refresh(sessionId);
  }, [refreshKey, sessionId]);

  return {
    trace,
    loading,
    error,
  };
}
