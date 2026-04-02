export interface ToolCall {
  index: number;
  tool_use_id: string;
  name: string;
  input: unknown;
  output: string;
  is_error: boolean;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  cost_usd: number;
}

export interface SessionRecord {
  id: string;
  file_path: string;
  created_at_ms: number;
  updated_at_ms: number;
  task: string;
}

export interface SessionSummary extends SessionRecord {
  total_cost_usd: number;
  status: string;
  tool_call_count: number;
}

export interface SessionDetail extends SessionRecord {
  total_cost_usd: number;
  status: string;
  tool_calls: ToolCall[];
}

export interface SessionTraceResponse {
  session: SessionDetail;
  tool_calls: ToolCall[];
}

export interface SessionUpdatedEvent {
  type: string;
  session_id: string;
  updated_at_ms: number;
}

export interface DiffLine {
  kind: "add" | "del" | "ctx";
  text: string;
}

export interface SessionStats {
  todayCost: number;
  sessionCount: number;
  toolCallCount: number;
}
