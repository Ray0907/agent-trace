import { startTransition, useDeferredValue, useEffect, useState } from "react";

import { DetailPanel } from "./components/DetailPanel";
import { SessionList } from "./components/SessionList";
import { Timeline } from "./components/Timeline";
import { useSessions } from "./hooks/useSessions";
import { useTrace } from "./hooks/useTrace";
import type { SessionSummary } from "./types";

export default function App() {
  const { sessions, loading: sessionsLoading, error: sessionsError } = useSessions();
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [selectedToolIndex, setSelectedToolIndex] = useState(0);
  const [actionMessage, setActionMessage] = useState<string | null>(null);

  useEffect(() => {
    if (sessions.length === 0) {
      setSelectedSessionId(null);
      return;
    }

    if (selectedSessionId && sessions.some((session) => session.id === selectedSessionId)) {
      return;
    }

    startTransition(() => {
      setSelectedSessionId(sessions[0].id);
      setSelectedToolIndex(0);
    });
  }, [selectedSessionId, sessions]);

  const activeSession =
    sessions.find((session) => session.id === selectedSessionId) ?? null;
  const { trace, loading: traceLoading, error: traceError } = useTrace(
    selectedSessionId,
    activeSession?.updated_at_ms ?? 0,
  );
  const deferredToolCalls = useDeferredValue(trace?.tool_calls ?? []);
  const selectedToolCall = deferredToolCalls[selectedToolIndex] ?? null;

  useEffect(() => {
    if (deferredToolCalls.length === 0) {
      setSelectedToolIndex(0);
      return;
    }

    if (!deferredToolCalls[selectedToolIndex]) {
      startTransition(() => {
        setSelectedToolIndex(0);
      });
    }
  }, [deferredToolCalls, selectedToolIndex]);

  useEffect(() => {
    setActionMessage(null);
  }, [selectedSessionId, selectedToolIndex]);

  const handleSessionSelect = (sessionId: string) => {
    startTransition(() => {
      setSelectedSessionId(sessionId);
      setSelectedToolIndex(0);
    });
  };

  const handleToolSelect = (index: number) => {
    startTransition(() => {
      setSelectedToolIndex(index);
    });
  };

  const handleReplay = async () => {
    if (!selectedToolCall) {
      return;
    }

    await copyJson(selectedToolCall.input, "Copied tool input to clipboard.", setActionMessage);
  };

  const handleFork = async () => {
    if (!activeSession || !selectedToolCall) {
      return;
    }

    await copyJson(
      {
        session_id: activeSession.id,
        tool_index: selectedToolCall.index,
        tool_use_id: selectedToolCall.tool_use_id,
        input: selectedToolCall.input,
      },
      "Copied fork checkpoint to clipboard.",
      setActionMessage,
    );
  };

  const handleExport = () => {
    const payload = selectedToolCall ?? trace;
    if (!payload) {
      return;
    }

    const blob = new Blob([JSON.stringify(payload, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = selectedToolCall
      ? `${selectedToolCall.tool_use_id}.json`
      : `${activeSession?.id ?? "trace"}.json`;
    anchor.click();
    URL.revokeObjectURL(url);
    setActionMessage("Exported JSON snapshot.");
  };

  const detailSession = activeSession;

  return (
    <div className="app-shell">
      <SessionList
        error={sessionsError}
        loading={sessionsLoading}
        onSelect={handleSessionSelect}
        selectedId={selectedSessionId}
        sessions={sessions}
      />
      <Timeline
        error={traceError}
        loading={traceLoading}
        onSelect={handleToolSelect}
        selectedIndex={selectedToolIndex}
        session={detailSession}
        toolCalls={deferredToolCalls}
      />
      <DetailPanel
        actionMessage={actionMessage}
        onExport={handleExport}
        onFork={handleFork}
        onReplay={handleReplay}
        session={detailSession}
        toolCall={selectedToolCall}
        toolCalls={deferredToolCalls}
      />
    </div>
  );
}

async function copyJson(
  value: unknown,
  successMessage: string,
  setActionMessage: (message: string) => void,
) {
  try {
    await navigator.clipboard.writeText(JSON.stringify(value, null, 2));
    setActionMessage(successMessage);
  } catch {
    setActionMessage("Clipboard access is unavailable in this context.");
  }
}
