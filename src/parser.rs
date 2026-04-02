use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::de::Deserializer;
use serde::Deserialize;
use serde_json::Value;

use crate::cost::{detect_pricing_from_path, estimate_cost_usd, ModelPricing, UsageTokens};
use crate::state::{Session, ToolCall};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsonlRecord {
    SessionMeta {
        version: u32,
        session_id: String,
        created_at_ms: u64,
        updated_at_ms: u64,
    },
    Message {
        message: RawMessage,
    },
    Compaction {
        count: u32,
        removed_message_count: usize,
        summary: String,
    },
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    role: String,
    #[serde(default)]
    blocks: Vec<RawBlock>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        tool_name: String,
        #[serde(default, deserialize_with = "deserialize_tool_output")]
        output: String,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

#[derive(Debug)]
struct SessionMetaRecord {
    session_id: String,
    created_at_ms: u64,
    updated_at_ms: u64,
}

pub fn parse_session_file(path: &Path) -> Result<Session> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read session file {}", path.display()))?;
    let pricing = detect_pricing_from_path(path);

    let mut meta: Option<SessionMetaRecord> = None;
    let mut task = String::new();
    let mut tool_calls = Vec::new();
    let mut pending_tool_calls = HashMap::<String, usize>::new();

    for (line_index, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let record: JsonlRecord = serde_json::from_str(trimmed).with_context(|| {
            format!(
                "invalid JSONL record on line {} in {}",
                line_index + 1,
                path.display()
            )
        })?;

        match record {
            JsonlRecord::SessionMeta {
                version,
                session_id,
                created_at_ms,
                updated_at_ms,
            } => {
                if version != 1 {
                    return Err(anyhow!(
                        "unsupported session_meta version {version} in {}",
                        path.display()
                    ));
                }
                if meta.is_some() {
                    return Err(anyhow!(
                        "duplicate session_meta record in {}",
                        path.display()
                    ));
                }
                meta = Some(SessionMetaRecord {
                    session_id,
                    created_at_ms,
                    updated_at_ms,
                });
            }
            JsonlRecord::Message { message } => {
                if meta.is_none() {
                    return Err(anyhow!(
                        "message record encountered before session_meta in {}",
                        path.display()
                    ));
                }
                process_message(
                    message,
                    &mut task,
                    &mut tool_calls,
                    &mut pending_tool_calls,
                    pricing,
                );
            }
            JsonlRecord::Compaction {
                count,
                removed_message_count,
                summary,
            } => {
                if meta.is_none() {
                    return Err(anyhow!(
                        "compaction record encountered before session_meta in {}",
                        path.display()
                    ));
                }
                let _ = (count, removed_message_count, summary);
            }
        }
    }

    let meta = meta.ok_or_else(|| anyhow!("missing session_meta record in {}", path.display()))?;
    let status = if tool_calls.iter().any(|call| call.is_error) {
        "error"
    } else if pending_tool_calls.is_empty() {
        "ok"
    } else {
        "running"
    };
    let total_cost_usd = tool_calls.iter().map(|call| call.cost_usd).sum();

    Ok(Session {
        id: meta.session_id,
        file_path: path.display().to_string(),
        created_at_ms: meta.created_at_ms,
        updated_at_ms: meta.updated_at_ms,
        task,
        tool_calls,
        total_cost_usd,
        status: status.to_string(),
    })
}

fn process_message(
    message: RawMessage,
    task: &mut String,
    tool_calls: &mut Vec<ToolCall>,
    pending_tool_calls: &mut HashMap<String, usize>,
    pricing: ModelPricing,
) {
    if message.role == "user" && task.is_empty() {
        if let Some(first_text) = message.blocks.iter().find_map(|block| match block {
            RawBlock::Text { text } => Some(text.clone()),
            _ => None,
        }) {
            *task = first_text;
        }
    }

    match message.role.as_str() {
        "assistant" => process_assistant_message(message, tool_calls, pending_tool_calls, pricing),
        "tool" => process_tool_message(message, tool_calls, pending_tool_calls),
        _ => {}
    }
}

fn process_assistant_message(
    message: RawMessage,
    tool_calls: &mut Vec<ToolCall>,
    pending_tool_calls: &mut HashMap<String, usize>,
    pricing: ModelPricing,
) {
    let tool_use_blocks: Vec<(String, String, String)> = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            RawBlock::ToolUse { id, name, input } => {
                Some((id.clone(), name.clone(), input.clone()))
            }
            _ => None,
        })
        .collect();
    let usage_slices = split_usage(message.usage.unwrap_or_default(), tool_use_blocks.len());

    for (offset, (tool_use_id, name, input)) in tool_use_blocks.into_iter().enumerate() {
        let usage = usage_slices.get(offset).copied().unwrap_or_default();
        let index = tool_calls.len();

        tool_calls.push(ToolCall {
            index,
            tool_use_id: tool_use_id.clone(),
            name,
            input: parse_tool_input(&input),
            output: String::new(),
            is_error: false,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            cost_usd: estimate_cost_usd(usage, pricing),
        });

        pending_tool_calls.insert(tool_use_id, index);
    }
}

fn process_tool_message(
    message: RawMessage,
    tool_calls: &mut [ToolCall],
    pending_tool_calls: &mut HashMap<String, usize>,
) {
    for block in message.blocks {
        if let RawBlock::ToolResult {
            tool_use_id,
            tool_name,
            output,
            is_error,
        } = block
        {
            let _ = tool_name;
            if let Some(index) = pending_tool_calls.remove(&tool_use_id) {
                if let Some(tool_call) = tool_calls.get_mut(index) {
                    tool_call.output = output;
                    tool_call.is_error = is_error;
                }
            }
        }
    }
}

fn parse_tool_input(input: &str) -> Value {
    serde_json::from_str(input).unwrap_or_else(|_| Value::String(input.to_string()))
}

fn deserialize_tool_output<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;

    Ok(match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text,
        Some(other) => other.to_string(),
    })
}

fn split_usage(usage: RawUsage, count: usize) -> Vec<UsageTokens> {
    if count == 0 {
        return Vec::new();
    }

    let input_tokens = split_token_count(usage.input_tokens, count);
    let output_tokens = split_token_count(usage.output_tokens, count);
    let cache_write_tokens = split_token_count(usage.cache_creation_input_tokens, count);
    let cache_read_tokens = split_token_count(usage.cache_read_input_tokens, count);

    (0..count)
        .map(|index| UsageTokens {
            input_tokens: input_tokens[index],
            output_tokens: output_tokens[index],
            cache_write_tokens: cache_write_tokens[index],
            cache_read_tokens: cache_read_tokens[index],
        })
        .collect()
}

fn split_token_count(total: u32, count: usize) -> Vec<u32> {
    let divisor = count as u32;
    let base = total / divisor;
    let remainder = total % divisor;

    (0..count)
        .map(|index| base + u32::from((index as u32) < remainder))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use crate::parser::parse_session_file;

    static TEMP_SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn write_temp_session(contents: &str) -> PathBuf {
        loop {
            let mut path = std::env::temp_dir();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("valid clock")
                .as_nanos();
            let counter = TEMP_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
            path.push(format!(
                "agent-trace-parser-{}-{nanos}-{counter}.jsonl",
                std::process::id()
            ));

            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(contents.as_bytes())
                        .expect("temp session should be written");
                    return fs::canonicalize(&path).unwrap_or(path);
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("temp session should be written: {err}"),
            }
        }
    }

    #[test]
    fn parses_meta_messages_and_compaction() {
        let path = write_temp_session(
            r#"{"type":"session_meta","version":1,"session_id":"session-1","created_at_ms":1000,"updated_at_ms":2000}
{"type":"message","message":{"role":"user","blocks":[{"type":"text","text":"Investigate build failure"}],"usage":null}}
{"type":"message","message":{"role":"assistant","blocks":[{"type":"text","text":"Running a command"},{"type":"tool_use","id":"tu_1","name":"Bash","input":"{\"command\":\"ls\"}"}],"usage":{"input_tokens":1200,"output_tokens":80,"cache_creation_input_tokens":0,"cache_read_input_tokens":900}}}
{"type":"message","message":{"role":"tool","blocks":[{"type":"tool_result","tool_use_id":"tu_1","tool_name":"Bash","output":"file1.ts\nfile2.ts","is_error":false}],"usage":null}}
{"type":"compaction","count":1,"removed_message_count":15,"summary":"trimmed"}"#,
        );

        let session = parse_session_file(&path).expect("session should parse");

        assert_eq!(session.id, "session-1");
        assert_eq!(session.task, "Investigate build failure");
        assert_eq!(session.tool_calls.len(), 1);
        assert_eq!(session.tool_calls[0].tool_use_id, "tu_1");
        assert_eq!(session.tool_calls[0].name, "Bash");
        assert_eq!(session.tool_calls[0].input, json!({ "command": "ls" }));
        assert_eq!(session.tool_calls[0].output, "file1.ts\nfile2.ts");
        assert_eq!(session.tool_calls[0].input_tokens, 1200);
        assert_eq!(session.tool_calls[0].output_tokens, 80);
        assert_eq!(session.tool_calls[0].cache_write_tokens, 0);
        assert_eq!(session.tool_calls[0].cache_read_tokens, 900);
        assert_eq!(session.status, "ok");

        fs::remove_file(path).expect("cleanup should succeed");
    }

    #[test]
    fn marks_session_running_when_tool_result_is_missing() {
        let path = write_temp_session(
            r#"{"type":"session_meta","version":1,"session_id":"session-2","created_at_ms":1000,"updated_at_ms":2000}
{"type":"message","message":{"role":"user","blocks":[{"type":"text","text":"Do work"}],"usage":null}}
{"type":"message","message":{"role":"assistant","blocks":[{"type":"tool_use","id":"tu_2","name":"Read","input":"{\"file_path\":\"README.md\"}"}],"usage":{"input_tokens":10,"output_tokens":20,"cache_creation_input_tokens":30,"cache_read_input_tokens":40}}}"#,
        );

        let session = parse_session_file(&path).expect("session should parse");

        assert_eq!(session.status, "running");
        assert_eq!(session.tool_calls.len(), 1);
        assert_eq!(session.tool_calls[0].output, "");

        fs::remove_file(path).expect("cleanup should succeed");
    }

    #[test]
    fn splits_usage_evenly_across_multiple_tool_calls() {
        let path = write_temp_session(
            r#"{"type":"session_meta","version":1,"session_id":"session-3","created_at_ms":1000,"updated_at_ms":2000}
{"type":"message","message":{"role":"user","blocks":[{"type":"text","text":"Do more work"}],"usage":null}}
{"type":"message","message":{"role":"assistant","blocks":[{"type":"tool_use","id":"tu_3","name":"Read","input":"{\"file_path\":\"a.txt\"}"},{"type":"tool_use","id":"tu_4","name":"Read","input":"{\"file_path\":\"b.txt\"}"}],"usage":{"input_tokens":11,"output_tokens":5,"cache_creation_input_tokens":7,"cache_read_input_tokens":3}}}
{"type":"message","message":{"role":"tool","blocks":[{"type":"tool_result","tool_use_id":"tu_3","tool_name":"Read","output":"A","is_error":false},{"type":"tool_result","tool_use_id":"tu_4","tool_name":"Read","output":"B","is_error":false}],"usage":null}}"#,
        );

        let session = parse_session_file(&path).expect("session should parse");

        assert_eq!(session.tool_calls.len(), 2);
        assert_eq!(session.tool_calls[0].input_tokens, 6);
        assert_eq!(session.tool_calls[1].input_tokens, 5);
        assert_eq!(session.tool_calls[0].output_tokens, 3);
        assert_eq!(session.tool_calls[1].output_tokens, 2);
        assert_eq!(session.tool_calls[0].cache_write_tokens, 4);
        assert_eq!(session.tool_calls[1].cache_write_tokens, 3);
        assert_eq!(session.tool_calls[0].cache_read_tokens, 2);
        assert_eq!(session.tool_calls[1].cache_read_tokens, 1);

        fs::remove_file(path).expect("cleanup should succeed");
    }

    #[test]
    fn marks_session_error_when_tool_result_reports_failure() {
        let path = write_temp_session(
            r#"{"type":"session_meta","version":1,"session_id":"session-4","created_at_ms":1000,"updated_at_ms":2000}
{"type":"message","message":{"role":"user","blocks":[{"type":"text","text":"Fail work"}],"usage":null}}
{"type":"message","message":{"role":"assistant","blocks":[{"type":"tool_use","id":"tu_5","name":"Bash","input":"{\"command\":\"exit 1\"}"}],"usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
{"type":"message","message":{"role":"tool","blocks":[{"type":"tool_result","tool_use_id":"tu_5","tool_name":"Bash","output":"","is_error":true}],"usage":null}}"#,
        );

        let session = parse_session_file(&path).expect("session should parse");

        assert_eq!(session.status, "error");
        assert!(session.total_cost_usd > 0.0);

        fs::remove_file(path).expect("cleanup should succeed");
    }

    #[test]
    fn marks_session_error_when_tool_result_output_is_null() {
        let path = write_temp_session(
            r#"{"type":"session_meta","version":1,"session_id":"session-6","created_at_ms":1000,"updated_at_ms":2000}
{"type":"message","message":{"role":"user","blocks":[{"type":"text","text":"Fail work quietly"}],"usage":null}}
{"type":"message","message":{"role":"assistant","blocks":[{"type":"tool_use","id":"tu_7","name":"Bash","input":"{\"command\":\"exit 1\"}"}],"usage":{"input_tokens":100,"output_tokens":25,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
{"type":"message","message":{"role":"tool","blocks":[{"type":"tool_result","tool_use_id":"tu_7","tool_name":"Bash","output":null,"is_error":true}],"usage":null}}"#,
        );

        let session = parse_session_file(&path).expect("session should parse");

        assert_eq!(session.status, "error");
        assert_eq!(session.tool_calls[0].output, "");

        fs::remove_file(path).expect("cleanup should succeed");
    }

    #[test]
    fn empty_tool_output_is_still_complete_when_result_arrives() {
        let path = write_temp_session(
            r#"{"type":"session_meta","version":1,"session_id":"session-5","created_at_ms":1000,"updated_at_ms":2000}
{"type":"message","message":{"role":"user","blocks":[{"type":"text","text":"Silent tool"}],"usage":null}}
{"type":"message","message":{"role":"assistant","blocks":[{"type":"tool_use","id":"tu_6","name":"Read","input":"{\"file_path\":\"empty.txt\"}"}],"usage":{"input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
{"type":"message","message":{"role":"tool","blocks":[{"type":"tool_result","tool_use_id":"tu_6","tool_name":"Read","output":"","is_error":false}],"usage":null}}"#,
        );

        let session = parse_session_file(&path).expect("session should parse");

        assert_eq!(session.status, "ok");

        fs::remove_file(path).expect("cleanup should succeed");
    }
}
