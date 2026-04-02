import type {
  DiffLine,
  SessionRecord,
  SessionStats,
  SessionSummary,
  SessionUpdatedEvent,
  ToolCall,
} from "../types";

type ToolTone = "blue" | "green" | "yellow" | "red" | "purple" | "aqua";
type ToolInputRecord = Record<string, unknown>;

const DATE_TIME_FORMATTER = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

export function deriveSessionStats(sessions: SessionSummary[], now = new Date()): SessionStats {
  const todayKey = dayKey(now.getTime());

  return sessions.reduce<SessionStats>(
    (stats, session) => {
      stats.sessionCount += 1;
      stats.toolCallCount += session.tool_call_count;

      if (dayKey(session.updated_at_ms) === todayKey) {
        stats.todayCost += session.total_cost_usd;
      }

      return stats;
    },
    {
      todayCost: 0,
      sessionCount: 0,
      toolCallCount: 0,
    },
  );
}

export function classifyTool(name: string): { label: string; tone: ToolTone } {
  const normalized = name.trim().toLowerCase();

  if (normalized.includes("read")) {
    return { label: "Read", tone: "blue" };
  }
  if (normalized.includes("write") || normalized.includes("create")) {
    return { label: "Write", tone: "green" };
  }
  if (
    normalized.includes("edit") ||
    normalized.includes("replace") ||
    normalized.includes("patch")
  ) {
    return { label: "Edit", tone: "yellow" };
  }
  if (
    normalized.includes("bash") ||
    normalized.includes("shell") ||
    normalized.includes("command")
  ) {
    return { label: "Bash", tone: "red" };
  }
  if (normalized.includes("agent")) {
    return { label: "Agent", tone: "purple" };
  }

  return {
    label: name.trim() || "Tool",
    tone: "aqua",
  };
}

export function formatCurrency(value: number): string {
  return `$${value.toFixed(3)}`;
}

export function formatTimestamp(timestampMs: number): string {
  if (!timestampMs) {
    return "No timestamp";
  }

  return DATE_TIME_FORMATTER.format(timestampMs);
}

export function formatCompactDuration(durationMs: number): string {
  if (durationMs <= 0) {
    return "0s";
  }

  const totalSeconds = Math.round(durationMs / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;

  if (minutes === 0) {
    return `${seconds}s`;
  }

  return `${minutes}m ${seconds}s`;
}

export function totalTokens(toolCall: ToolCall): number {
  return (
    toolCall.input_tokens +
    toolCall.output_tokens +
    toolCall.cache_read_tokens +
    toolCall.cache_write_tokens
  );
}

export function estimateLatencyMs(
  session: Pick<SessionRecord, "created_at_ms" | "updated_at_ms"> | null,
  toolCall: ToolCall,
  toolCalls: ToolCall[],
): number {
  if (!session || toolCalls.length === 0) {
    return 0;
  }

  const durationMs = Math.max(session.updated_at_ms - session.created_at_ms, toolCalls.length);
  const totalWeight = toolCalls.reduce((sum, call) => sum + totalTokens(call) + 1, 0);
  const callWeight = totalTokens(toolCall) + 1;

  return Math.max(1, Math.round((durationMs * callWeight) / totalWeight));
}

export function summarizeToolInput(input: unknown): string {
  if (typeof input === "string") {
    return compactText(input, 84);
  }

  if (!isRecord(input)) {
    return String(input ?? "");
  }

  const command = stringField(input, ["command"]);
  if (command) {
    return compactText(command, 84);
  }

  const prompt = stringField(input, ["prompt", "query", "pattern"]);
  if (prompt) {
    return compactText(prompt, 84);
  }

  const filePath = extractFilePath(input);
  if (filePath) {
    const lineHint = stringField(input, ["old_string", "new_string", "old_str", "new_str"]);
    return lineHint ? `${filePath} · ${compactText(lineHint, 48)}` : filePath;
  }

  return compactText(JSON.stringify(input), 84);
}

export function extractFilePath(input: unknown): string | null {
  if (!isRecord(input)) {
    return null;
  }

  return (
    stringField(input, [
      "file_path",
      "path",
      "target_file",
      "filename",
      "cwd",
      "working_directory",
    ]) ?? null
  );
}

export function buildDiffLines(toolCall: ToolCall): DiffLine[] {
  const input = isRecord(toolCall.input) ? toolCall.input : null;
  const patchText =
    stringField(input, ["patch", "diff", "unified_diff"]) ??
    (looksLikeDiff(toolCall.output) ? toolCall.output : null);

  if (patchText) {
    const parsed = parseUnifiedDiff(patchText);
    if (parsed.length > 0) {
      return parsed;
    }
  }

  const before =
    stringField(input, ["old_string", "old_str", "before", "original_text", "search"]) ?? "";
  const after =
    stringField(input, ["new_string", "new_str", "after", "replacement", "text"]) ?? "";

  if (before || after) {
    return diffLines(before, after);
  }

  const preview = input
    ? JSON.stringify(input, null, 2)
    : toolCall.output || "(No diff-like payload available)";

  return preview
    .split("\n")
    .slice(0, 32)
    .map((text) => ({
      kind: "ctx",
      text,
    }));
}

export function buildSessionSummary(
  session: SessionSummary | null,
  toolCalls: ToolCall[],
): string {
  if (!session) {
    return "No session selected";
  }

  return `${toolCalls.length || session.tool_call_count} calls · ${formatCompactDuration(
    Math.max(session.updated_at_ms - session.created_at_ms, 0),
  )} · ${formatCurrency(session.total_cost_usd)}`;
}

export function getTokenBars(toolCall: ToolCall) {
  const values = [
    {
      key: "input",
      label: "Input",
      value: toolCall.input_tokens,
      color: "var(--blue)",
    },
    {
      key: "output",
      label: "Output",
      value: toolCall.output_tokens,
      color: "var(--green)",
    },
    {
      key: "cache",
      label: "Cache",
      value: toolCall.cache_read_tokens + toolCall.cache_write_tokens,
      color: "var(--yellow)",
    },
  ] as const;
  const maxValue = Math.max(...values.map((entry) => entry.value), 1);

  return values.map((entry) => ({
    ...entry,
    width: `${Math.max((entry.value / maxValue) * 100, entry.value > 0 ? 10 : 0)}%`,
  }));
}

export function isSessionUpdatedEvent(value: unknown): value is SessionUpdatedEvent {
  return (
    typeof value === "object" &&
    value !== null &&
    (value as { type?: string }).type === "session_updated"
  );
}

function dayKey(timestampMs: number): string {
  const date = new Date(timestampMs);
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
}

function compactText(text: string, maxLength: number): string {
  const normalized = text.replace(/\s+/g, " ").trim();
  if (normalized.length <= maxLength) {
    return normalized;
  }

  return `${normalized.slice(0, maxLength - 1)}…`;
}

function isRecord(value: unknown): value is ToolInputRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringField(
  record: ToolInputRecord | null,
  fields: string[],
): string | null {
  if (!record) {
    return null;
  }

  for (const field of fields) {
    const value = record[field];
    if (typeof value === "string" && value.trim()) {
      return value;
    }
  }

  return null;
}

function looksLikeDiff(text: string): boolean {
  return text.split("\n").some((line) => {
    const trimmed = line.trimStart();
    return (
      trimmed.startsWith("@@") ||
      trimmed.startsWith("+") ||
      trimmed.startsWith("-")
    );
  });
}

function parseUnifiedDiff(text: string): DiffLine[] {
  const lines = text.split("\n");
  const diffLines: DiffLine[] = [];

  for (const line of lines) {
    if (!line || line.startsWith("diff ") || line.startsWith("index ")) {
      continue;
    }

    if (line.startsWith("+++ ") || line.startsWith("--- ") || line.startsWith("@@")) {
      diffLines.push({ kind: "ctx", text: line });
      continue;
    }

    if (line.startsWith("+")) {
      diffLines.push({ kind: "add", text: line.slice(1) });
      continue;
    }

    if (line.startsWith("-")) {
      diffLines.push({ kind: "del", text: line.slice(1) });
      continue;
    }

    diffLines.push({ kind: "ctx", text: line.startsWith(" ") ? line.slice(1) : line });
  }

  return diffLines;
}

function diffLines(before: string, after: string): DiffLine[] {
  const beforeLines = before.split("\n");
  const afterLines = after.split("\n");

  let prefixLength = 0;
  while (
    prefixLength < beforeLines.length &&
    prefixLength < afterLines.length &&
    beforeLines[prefixLength] === afterLines[prefixLength]
  ) {
    prefixLength += 1;
  }

  let suffixLength = 0;
  while (
    suffixLength + prefixLength < beforeLines.length &&
    suffixLength + prefixLength < afterLines.length &&
    beforeLines[beforeLines.length - 1 - suffixLength] ===
      afterLines[afterLines.length - 1 - suffixLength]
  ) {
    suffixLength += 1;
  }

  const diff: DiffLine[] = [];

  for (const line of beforeLines.slice(0, prefixLength)) {
    diff.push({ kind: "ctx", text: line });
  }

  for (const line of beforeLines.slice(prefixLength, beforeLines.length - suffixLength)) {
    diff.push({ kind: "del", text: line });
  }

  for (const line of afterLines.slice(prefixLength, afterLines.length - suffixLength)) {
    diff.push({ kind: "add", text: line });
  }

  for (const line of beforeLines.slice(beforeLines.length - suffixLength)) {
    diff.push({ kind: "ctx", text: line });
  }

  return diff;
}
