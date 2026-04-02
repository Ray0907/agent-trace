import {
  deriveSessionStats,
  formatCurrency,
  formatTimestamp,
} from "../lib/trace-ui";
import type { SessionSummary } from "../types";

interface SessionListProps {
  sessions: SessionSummary[];
  selectedId: string | null;
  loading: boolean;
  error: string | null;
  onSelect: (sessionId: string) => void;
}

export function SessionList({
  sessions,
  selectedId,
  loading,
  error,
  onSelect,
}: SessionListProps) {
  const stats = deriveSessionStats(sessions);

  return (
    <aside className="panel panel-sessions">
      <header className="panel-header panel-header-stack">
        <span className="panel-kicker">SESSIONS</span>
        <span className="panel-note">
          {error ?? (loading ? "Loading live session index…" : `${sessions.length} sessions loaded`)}
        </span>
      </header>

      <div className="session-list">
        {sessions.map((session) => {
          const isActive = session.id === selectedId;

          return (
            <button
              key={session.id}
              className={`session-row${isActive ? " is-active" : ""}`}
              onClick={() => onSelect(session.id)}
              type="button"
            >
              <div className="session-row-top">
                <span className={`status-dot status-${session.status}`} />
                <span className="session-task">{session.task || "Untitled session"}</span>
                <span className="session-cost">{formatCurrency(session.total_cost_usd)}</span>
              </div>
              <div className="session-row-meta">
                <span className="truncate">{session.id}</span>
                <span>{formatTimestamp(session.updated_at_ms)}</span>
              </div>
            </button>
          );
        })}
        {!loading && sessions.length === 0 ? (
          <div className="panel-empty">No daemon sessions found.</div>
        ) : null}
      </div>

      <footer className="session-stats">
        <div>
          <span className="stats-label">Today</span>
          <span className="stats-value stats-accent">{formatCurrency(stats.todayCost)}</span>
        </div>
        <div>
          <span className="stats-label">Sessions</span>
          <span className="stats-value">{stats.sessionCount}</span>
        </div>
        <div>
          <span className="stats-label">Calls</span>
          <span className="stats-value">{stats.toolCallCount}</span>
        </div>
      </footer>
    </aside>
  );
}
