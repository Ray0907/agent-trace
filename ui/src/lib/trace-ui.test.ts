import { describe, expect, it } from "vitest";

import { buildDiffLines, deriveSessionStats, isSessionUpdatedEvent } from "./trace-ui";

import type { SessionSummary, ToolCall } from "../types";

describe("deriveSessionStats", () => {
  it("sums today cost and tool calls", () => {
    const sessions: SessionSummary[] = [
      {
        id: "a",
        file_path: "/tmp/a.jsonl",
        created_at_ms: 0,
        updated_at_ms: Date.UTC(2026, 3, 3, 2, 0, 0),
        task: "Inspect logs",
        total_cost_usd: 1.25,
        status: "ok",
        tool_call_count: 3,
      },
      {
        id: "b",
        file_path: "/tmp/b.jsonl",
        created_at_ms: 0,
        updated_at_ms: Date.UTC(2026, 2, 31, 12, 0, 0),
        task: "Patch app",
        total_cost_usd: 0.75,
        status: "running",
        tool_call_count: 5,
      },
    ];

    expect(
      deriveSessionStats(sessions, new Date(Date.UTC(2026, 3, 3, 8, 0, 0))),
    ).toEqual({
      todayCost: 1.25,
      sessionCount: 2,
      toolCallCount: 8,
    });
  });
});

describe("buildDiffLines", () => {
  it("renders a line diff from old/new string tool inputs", () => {
    const toolCall: ToolCall = {
      index: 2,
      tool_use_id: "tool-2",
      name: "Edit",
      input: {
        file_path: "/tmp/app.ts",
        old_string: "before\nshared",
        new_string: "after\nshared",
      },
      output: "",
      is_error: false,
      input_tokens: 100,
      output_tokens: 40,
      cache_read_tokens: 10,
      cache_write_tokens: 0,
      cost_usd: 0.002,
    };

    expect(buildDiffLines(toolCall)).toEqual([
      { kind: "del", text: "before" },
      { kind: "add", text: "after" },
      { kind: "ctx", text: "shared" },
    ]);
  });
});

describe("isSessionUpdatedEvent", () => {
  it("accepts daemon refresh events", () => {
    expect(
      isSessionUpdatedEvent({
        type: "session_updated",
        session_id: "abc",
        updated_at_ms: 123,
      }),
    ).toBe(true);
  });
});
