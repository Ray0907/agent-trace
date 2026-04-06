use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde::de::Deserializer;
use serde::Deserialize;
use serde_json::Value;

use crate::cost::{detect_pricing_from_path, estimate_cost_usd, ModelPricing, UsageTokens};
use crate::state::{Session, ToolCall};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JsonlRecord {
    Legacy(LegacyRecord),
    Real(RawRecord),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LegacyRecord {
    SessionMeta {
        version: u32,
        session_id: String,
        created_at_ms: u64,
        updated_at_ms: u64,
    },
    Message {
        message: LegacyMessage,
    },
    Compaction {
        count: u32,
        removed_message_count: usize,
        summary: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawRecord {
    User {
        message: RawMessage,
    },
    Assistant {
        message: RawMessage,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    role: String,
    #[serde(default, deserialize_with = "deserialize_content")]
    content: Vec<RawContent>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

fn deserialize_content<'de, D>(deserializer: D) -> Result<Vec<RawContent>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ContentField {
        Array(Vec<RawContent>),
        String(String),
        Null,
    }
    match ContentField::deserialize(deserializer)? {
        ContentField::Array(v) => Ok(v),
        _ => Ok(Vec::new()),
    }
}

fn deserialize_tool_result_content<'de, D>(deserializer: D) -> Result<Vec<TextContent>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum TrContent {
        Array(Vec<TextContent>),
        String(String),
        Null,
    }
    match TrContent::deserialize(deserializer)? {
        TrContent::Array(v) => Ok(v),
        TrContent::String(s) => Ok(vec![TextContent { r#type: "text".to_string(), text: s }]),
        TrContent::Null => Ok(Vec::new()),
    }
}

#[derive(Debug, Deserialize)]
struct LegacyMessage {
    role: String,
    #[serde(default)]
    blocks: Vec<LegacyBlock>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawContent {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default, deserialize_with = "deserialize_tool_result_content")]
        content: Vec<TextContent>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LegacyBlock {
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

#[derive(Debug, Deserialize)]
struct TextContent {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    text: String,
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

pub fn parse_session_file(path: &Path) -> Result<Session> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read session file {}", path.display()))?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read session metadata {}", path.display()))?;
    let pricing = detect_pricing_from_path(path);
    let (default_created_at_ms, default_updated_at_ms) = metadata_timestamps_ms(&metadata)?;

    let mut session_id = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("session file has no valid stem: {}", path.display()))?
        .to_string();
    let mut created_at_ms = default_created_at_ms;
    let mut updated_at_ms = default_updated_at_ms;
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
            JsonlRecord::Legacy(record) => match record {
                LegacyRecord::SessionMeta {
                    version,
                    session_id: legacy_session_id,
                    created_at_ms: legacy_created_at_ms,
                    updated_at_ms: legacy_updated_at_ms,
                } => {
                    if version != 1 {
                        return Err(anyhow!(
                            "unsupported session_meta version {version} in {}",
                            path.display()
                        ));
                    }

                    session_id = legacy_session_id;
                    created_at_ms = legacy_created_at_ms;
                    updated_at_ms = legacy_updated_at_ms;
                }
                LegacyRecord::Message { message } => {
                    process_legacy_message(
                        message,
                        &mut task,
                        &mut tool_calls,
                        &mut pending_tool_calls,
                        pricing,
                    );
                }
                LegacyRecord::Compaction {
                    count,
                    removed_message_count,
                    summary,
                } => {
                    let _ = (count, removed_message_count, summary);
                }
            },
            JsonlRecord::Real(record) => match record {
                RawRecord::User { message } => process_user_message(
                    message,
                    &mut task,
                    &mut tool_calls,
                    &mut pending_tool_calls,
                ),
                RawRecord::Assistant { message } => process_assistant_message(
                    message,
                    &mut tool_calls,
                    &mut pending_tool_calls,
                    pricing,
                ),
                RawRecord::Unknown => {}
            },
        }
    }

    let status = if tool_calls.iter().any(|call| call.is_error) {
        "error"
    } else if pending_tool_calls.is_empty() {
        "ok"
    } else {
        "running"
    };
    let total_cost_usd = tool_calls.iter().map(|call| call.cost_usd).sum();

    Ok(Session {
        id: session_id,
        file_path: path.display().to_string(),
        created_at_ms,
        updated_at_ms,
        task,
        tool_calls,
        total_cost_usd,
        status: status.to_string(),
    })
}

fn process_legacy_message(
    message: LegacyMessage,
    task: &mut String,
    tool_calls: &mut Vec<ToolCall>,
    pending_tool_calls: &mut HashMap<String, usize>,
    pricing: ModelPricing,
) {
    if message.role == "user" && task.is_empty() {
        if let Some(first_text) = message.blocks.iter().find_map(|block| match block {
            LegacyBlock::Text { text } => Some(text.clone()),
            _ => None,
        }) {
            *task = first_text;
        }
    }

    match message.role.as_str() {
        "assistant" => {
            process_legacy_assistant_message(message, tool_calls, pending_tool_calls, pricing)
        }
        "tool" => process_legacy_tool_message(message, tool_calls, pending_tool_calls),
        _ => {}
    }
}

fn process_assistant_message(
    message: RawMessage,
    tool_calls: &mut Vec<ToolCall>,
    pending_tool_calls: &mut HashMap<String, usize>,
    pricing: ModelPricing,
) {
    let _ = &message.role;
    let tool_use_blocks: Vec<(String, String, Value)> = message
        .content
        .iter()
        .filter_map(|block| match block {
            RawContent::ToolUse { id, name, input } => {
                Some((id.clone(), name.clone(), input.clone()))
            }
            _ => None,
        })
        .collect();
    let usage_slices = split_usage(message.usage.unwrap_or_default(), tool_use_blocks.len());

    for (offset, (tool_use_id, name, input)) in tool_use_blocks.into_iter().enumerate() {
        let usage = usage_slices.get(offset).copied().unwrap_or_default();
        let index = push_tool_call(tool_calls, &tool_use_id, name, input, usage, pricing);
        pending_tool_calls.insert(tool_use_id, index);
    }
}

fn process_user_message(
    message: RawMessage,
    task: &mut String,
    tool_calls: &mut [ToolCall],
    pending_tool_calls: &mut HashMap<String, usize>,
) {
    if task.is_empty() {
        if let Some(first_text) = message.content.iter().find_map(|block| match block {
            RawContent::Text { text } => Some(text.clone()),
            _ => None,
        }) {
            *task = first_text;
        }
    }

    for block in message.content {
        if let RawContent::ToolResult {
            tool_use_id,
            content,
        } = block
        {
            let output = join_tool_result_content(&content);
            let is_error = detect_tool_result_error(&content, &output);
            if let Some(index) = pending_tool_calls.remove(&tool_use_id) {
                if let Some(tool_call) = tool_calls.get_mut(index) {
                    tool_call.output = output;
                    tool_call.is_error = is_error;
                }
            }
        }
    }
}

fn process_legacy_assistant_message(
    message: LegacyMessage,
    tool_calls: &mut Vec<ToolCall>,
    pending_tool_calls: &mut HashMap<String, usize>,
    pricing: ModelPricing,
) {
    let tool_use_blocks: Vec<(String, String, Value)> = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            LegacyBlock::ToolUse { id, name, input } => {
                Some((id.clone(), name.clone(), parse_tool_input(input)))
            }
            _ => None,
        })
        .collect();
    let usage_slices = split_usage(message.usage.unwrap_or_default(), tool_use_blocks.len());

    for (offset, (tool_use_id, name, input)) in tool_use_blocks.into_iter().enumerate() {
        let usage = usage_slices.get(offset).copied().unwrap_or_default();
        let index = push_tool_call(tool_calls, &tool_use_id, name, input, usage, pricing);
        pending_tool_calls.insert(tool_use_id, index);
    }
}

fn process_legacy_tool_message(
    message: LegacyMessage,
    tool_calls: &mut [ToolCall],
    pending_tool_calls: &mut HashMap<String, usize>,
) {
    for block in message.blocks {
        if let LegacyBlock::ToolResult {
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

fn push_tool_call(
    tool_calls: &mut Vec<ToolCall>,
    tool_use_id: &str,
    name: String,
    input: Value,
    usage: UsageTokens,
    pricing: ModelPricing,
) -> usize {
    let index = tool_calls.len();

    tool_calls.push(ToolCall {
        index,
        tool_use_id: tool_use_id.to_string(),
        name,
        input,
        output: String::new(),
        is_error: false,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        cost_usd: estimate_cost_usd(usage, pricing),
    });

    index
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

fn metadata_timestamps_ms(metadata: &fs::Metadata) -> Result<(u64, u64)> {
    let updated_at_ms = system_time_ms(
        metadata
            .modified()
            .context("failed to read file modification time")?,
    )?;
    let created_at_ms = match metadata.created() {
        Ok(created_at) => system_time_ms(created_at)?,
        Err(_) => updated_at_ms,
    };

    Ok((created_at_ms, updated_at_ms))
}

fn system_time_ms(value: SystemTime) -> Result<u64> {
    Ok(value
        .duration_since(UNIX_EPOCH)
        .map_err(|_| anyhow!("file timestamp is before unix epoch"))?
        .as_millis() as u64)
}

fn join_tool_result_content(content: &[TextContent]) -> String {
    content
        .iter()
        .filter(|entry| entry.r#type == "text" || entry.r#type.is_empty())
        .map(|entry| entry.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn detect_tool_result_error(content: &[TextContent], output: &str) -> bool {
    output.trim_start().starts_with("Error")
        || content
            .iter()
            .any(|entry| entry.text.trim_start().starts_with("Error"))
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

    fn system_time_ms(value: SystemTime) -> u64 {
        value
            .duration_since(UNIX_EPOCH)
            .expect("valid timestamp")
            .as_millis() as u64
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

    #[test]
    fn parses_real_claude_code_project_format() {
        let path = write_temp_session(
            r#"{"parentUuid":"root","type":"summary","summary":"skip me"}
{"parentUuid":"msg-1","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_old","content":[{"type":"text","text":"previous output"}]},{"type":"text","text":"Investigate why parser misses Claude sessions"}]}}
{"parentUuid":"msg-2","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls","cwd":"/tmp"}},{"type":"text","text":"Listing files"},{"type":"tool_use","id":"toolu_2","name":"Read","input":{"file_path":"src/parser.rs"}}],"usage":{"input_tokens":11,"output_tokens":5,"cache_creation_input_tokens":7,"cache_read_input_tokens":3}}}
{"parentUuid":"msg-3","type":"permission-mode","mode":"acceptEdits"}
{"parentUuid":"msg-4","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"file1"},{"type":"text","text":"file2"}]},{"type":"tool_result","tool_use_id":"toolu_2","content":[{"type":"text","text":"Error: missing file"}]}]}}"#,
        );
        let metadata = fs::metadata(&path).expect("metadata should be readable");

        let session = parse_session_file(&path).expect("session should parse");

        assert_eq!(
            session.id,
            path.file_stem()
                .and_then(|value| value.to_str())
                .expect("temp file stem should be valid")
        );
        assert_eq!(
            session.task,
            "Investigate why parser misses Claude sessions"
        );
        assert_eq!(
            session.created_at_ms,
            system_time_ms(
                metadata
                    .created()
                    .unwrap_or_else(|_| metadata.modified().expect("mtime"))
            )
        );
        assert_eq!(
            session.updated_at_ms,
            system_time_ms(metadata.modified().expect("mtime should exist"))
        );
        assert_eq!(session.tool_calls.len(), 2);
        assert_eq!(session.tool_calls[0].tool_use_id, "toolu_1");
        assert_eq!(
            session.tool_calls[0].input,
            json!({ "command": "ls", "cwd": "/tmp" })
        );
        assert_eq!(session.tool_calls[0].output, "file1\nfile2");
        assert!(!session.tool_calls[0].is_error);
        assert_eq!(session.tool_calls[1].tool_use_id, "toolu_2");
        assert_eq!(
            session.tool_calls[1].input,
            json!({ "file_path": "src/parser.rs" })
        );
        assert_eq!(session.tool_calls[1].output, "Error: missing file");
        assert!(session.tool_calls[1].is_error);
        assert_eq!(session.tool_calls[0].input_tokens, 6);
        assert_eq!(session.tool_calls[1].input_tokens, 5);
        assert_eq!(session.tool_calls[0].output_tokens, 3);
        assert_eq!(session.tool_calls[1].output_tokens, 2);
        assert_eq!(session.tool_calls[0].cache_write_tokens, 4);
        assert_eq!(session.tool_calls[1].cache_write_tokens, 3);
        assert_eq!(session.tool_calls[0].cache_read_tokens, 2);
        assert_eq!(session.tool_calls[1].cache_read_tokens, 1);
        assert_eq!(session.status, "error");

        fs::remove_file(path).expect("cleanup should succeed");
    }
}
