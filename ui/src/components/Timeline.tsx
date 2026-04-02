import {
  buildSessionSummary,
  classifyTool,
  estimateLatencyMs,
  formatCompactDuration,
  formatCurrency,
  summarizeToolInput,
  totalTokens,
} from "../lib/trace-ui";
import type { SessionSummary, ToolCall } from "../types";

interface TimelineProps {
  session: SessionSummary | null;
  toolCalls: ToolCall[];
  selectedIndex: number;
  loading: boolean;
  error: string | null;
  onSelect: (index: number) => void;
}

export function Timeline({
  session,
  toolCalls,
  selectedIndex,
  loading,
  error,
  onSelect,
}: TimelineProps) {
  const maxTokens = Math.max(...toolCalls.map(totalTokens), 1);

  return (
    <section className="panel panel-timeline">
      <header className="panel-header">
        <div>
          <h1 className="session-title">{session?.task || "Select a session"}</h1>
          <p className="panel-note">{error ?? buildSessionSummary(session, toolCalls)}</p>
        </div>
        {session ? (
          <div className="timeline-duration">
            {formatCompactDuration(
              Math.max(session.updated_at_ms - session.created_at_ms, 0),
            )}
          </div>
        ) : null}
      </header>

      <div className="timeline-list">
        {toolCalls.map((toolCall, rowIndex) => {
          const badge = classifyTool(toolCall.name);
          const estimatedLatencyMs = estimateLatencyMs(session, toolCall, toolCalls);

          return (
            <button
              key={toolCall.tool_use_id}
              className={`tool-row${selectedIndex === rowIndex ? " is-active" : ""}`}
              onClick={() => onSelect(rowIndex)}
              type="button"
            >
              <span className="tool-index">{String(toolCall.index + 1).padStart(2, "0")}</span>
              <span className={`tool-badge tone-${badge.tone}`}>{badge.label}</span>
              <span className="tool-arg">{summarizeToolInput(toolCall.input)}</span>
              <span className="tool-ms">{estimatedLatencyMs}ms</span>
              <span className="tool-cost">{formatCurrency(toolCall.cost_usd)}</span>
            </button>
          );
        })}
        {loading ? <div className="panel-empty">Loading trace…</div> : null}
        {!loading && session && toolCalls.length === 0 ? (
          <div className="panel-empty">This session does not contain any tool calls.</div>
        ) : null}
        {!loading && !session ? (
          <div className="panel-empty">Choose a session to inspect its trace.</div>
        ) : null}
      </div>

      <div className="mini-chart">
        <div className="mini-chart-header">
          <span className="panel-kicker">TOKENS</span>
          <span className="panel-note">Per turn usage</span>
        </div>
        <div className="mini-chart-bars">
          {toolCalls.map((toolCall, rowIndex) => (
            <button
              key={`${toolCall.tool_use_id}-bar`}
              className={`mini-bar${selectedIndex === rowIndex ? " is-active" : ""}`}
              onClick={() => onSelect(rowIndex)}
              style={{ height: `${Math.max((totalTokens(toolCall) / maxTokens) * 100, 8)}%` }}
              type="button"
            >
              <span className="sr-only">{toolCall.name}</span>
            </button>
          ))}
        </div>
      </div>
    </section>
  );
}
