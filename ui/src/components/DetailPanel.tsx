import {
  buildDiffLines,
  classifyTool,
  estimateLatencyMs,
  extractFilePath,
  formatCurrency,
  formatTimestamp,
  getTokenBars,
} from "../lib/trace-ui";
import type { SessionSummary, ToolCall } from "../types";

interface DetailPanelProps {
  session: SessionSummary | null;
  toolCall: ToolCall | null;
  toolCalls: ToolCall[];
  actionMessage: string | null;
  onReplay: () => void;
  onFork: () => void;
  onExport: () => void;
}

export function DetailPanel({
  session,
  toolCall,
  toolCalls,
  actionMessage,
  onReplay,
  onFork,
  onExport,
}: DetailPanelProps) {
  if (!session || !toolCall) {
    return (
      <aside className="panel panel-detail">
        <header className="panel-header panel-header-stack">
          <span className="panel-kicker">DETAILS</span>
          <span className="panel-note">Select a tool row to inspect it.</span>
        </header>
        <div className="panel-empty panel-empty-detail">
          Metadata, token bars, diff preview, and export actions appear here.
        </div>
      </aside>
    );
  }

  const badge = classifyTool(toolCall.name);
  const diffLines = buildDiffLines(toolCall);
  const filePath = extractFilePath(toolCall.input);
  const tokenBars = getTokenBars(toolCall);
  const estimatedLatencyMs = estimateLatencyMs(session, toolCall, toolCalls);

  return (
    <aside className="panel panel-detail">
      <header className="panel-header panel-header-stack">
        <span className="panel-kicker">DETAILS</span>
        <span className={`tool-badge tone-${badge.tone}`}>{badge.label}</span>
      </header>

      <section className="detail-section">
        <h2 className="section-label">METADATA</h2>
        <dl className="metadata-grid">
          <div>
            <dt>Tool</dt>
            <dd>{toolCall.name}</dd>
          </div>
          <div>
            <dt>File</dt>
            <dd>{filePath ?? "n/a"}</dd>
          </div>
          <div>
            <dt>Latency</dt>
            <dd>{estimatedLatencyMs}ms</dd>
          </div>
          <div>
            <dt>Cost</dt>
            <dd>{formatCurrency(toolCall.cost_usd)}</dd>
          </div>
          <div>
            <dt>Updated</dt>
            <dd>{formatTimestamp(session.updated_at_ms)}</dd>
          </div>
          <div>
            <dt>Status</dt>
            <dd>{toolCall.is_error ? "error" : session.status}</dd>
          </div>
        </dl>
      </section>

      <section className="detail-section">
        <h2 className="section-label">TOKENS</h2>
        <div className="token-bars">
          {tokenBars.map((tokenBar) => (
            <div key={tokenBar.key} className="token-row">
              <div className="token-row-top">
                <span>{tokenBar.label}</span>
                <span>{tokenBar.value}</span>
              </div>
              <div className="token-track">
                <div
                  className="token-fill"
                  style={{
                    width: tokenBar.width,
                    background: tokenBar.color,
                  }}
                />
              </div>
            </div>
          ))}
        </div>
      </section>

      <section className="detail-section detail-section-grow">
        <h2 className="section-label">DIFF</h2>
        <pre className="diff-block">
          {diffLines.map((line, index) => (
            <div key={`${line.kind}-${index}`} className={`diff-line diff-${line.kind}`}>
              <span className="diff-sign">
                {line.kind === "add" ? "+" : line.kind === "del" ? "-" : " "}
              </span>
              <span>{line.text || " "}</span>
            </div>
          ))}
        </pre>
      </section>

      <section className="detail-section">
        <h2 className="section-label">ACTIONS</h2>
        <div className="action-row">
          <button className="action-button" onClick={onReplay} type="button">
            Replay
          </button>
          <button className="action-button" onClick={onFork} type="button">
            Fork here
          </button>
          <button className="action-button" onClick={onExport} type="button">
            Export
          </button>
        </div>
        {actionMessage ? <p className="panel-note">{actionMessage}</p> : null}
      </section>
    </aside>
  );
}
