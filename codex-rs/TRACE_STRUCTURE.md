# Codex Tracing Structure

This document describes the OpenTelemetry tracing structure implemented in Codex to provide clean, hierarchical traces of user conversations and assistant actions.

## Overview

The tracing follows the exact structure requested:

```
codex_session (ROOT) ─┬─ user_message       role="user"   content="<text>"
                      ├─ llm_request        model, prompt_tokens, retries…
                      │   └─ assistant_msg  role="assistant" content="<text>"
                      ├─ tool_call          tool="computer.click" args=…
                      │   ├─ tool_execution status, exit_code, duration…
                      │   └─ exec_cmd       cmd="git diff" exit_code, stdout…
                      └─ exec_cmd           cmd="git diff" exit_code, stdout…
```

## Span Hierarchy

### Root Session Span

- **Name**:
  - `codex_session` (for headless exec mode)
  - `codex_tui_session` (for terminal UI)
  - `codex_proto_session` (for protocol/stdio mode)
- **Attributes**:
  - `git_commit`: Current git HEAD commit SHA
  - `git_repository_url`: Repository URL from Cargo.toml
  - `codex_config_model`: Model being used (exec mode only)
  - `codex_config_flags`: CLI flags passed (exec and TUI)
  - `codex_version`: Binary version
  - Token count fields (populated during conversation)

### User Message Spans

- **Name**: `user_message`
- **Attributes**:
  - `role`: "user"
  - `content`: User's message content (truncated to 64KB)
  - `message_type`: "user_input"

### LLM Request Spans

- **Name**: `llm_request`
- **Parent**: User message span
- **Attributes**:
  - `model`: Model name (e.g., "gpt-4o")
  - `provider`: Provider name (e.g., "openai")
  - `prompt_tokens`: Input token count
  - `completion_tokens`: Output token count
  - `total_tokens`: Total token count
  - `cached_tokens`: Cached input tokens (if available)
  - `reasoning_tokens`: Reasoning output tokens (if available)
  - `retries`: Number of retry attempts

### Assistant Message Spans

- **Name**: `assistant_msg`
- **Parent**: LLM request span
- **Attributes**:
  - `role`: "assistant"
  - `content`: Assistant's response content (truncated to 64KB)
  - `message_type`: "assistant_response"

### Tool Call Spans

- **Name**: `tool_call`
- **Parent**: User message span or LLM request span
- **Attributes**:
  - `tool`: Tool name (e.g., "shell", "apply_patch")
  - `args`: Tool arguments (truncated to 64KB)
  - `call_type`: "function_call"

### Command Execution Spans

- **Name**: `exec_cmd`
- **Parent**: Tool call span
- **Attributes**:
  - `cmd`: Command being executed
  - `exit_code`: Exit code of the command
  - `duration_ms`: Execution duration in milliseconds
  - `stdout_size`: Size of stdout output
  - `stderr_size`: Size of stderr output

## Noise Reduction

### What's Filtered Out

- Individual token deltas from streaming LLM responses
- Per-chunk SSE events from chat completions
- Debug-level tracing events
- Duplicate events from rollout recording

### What's Captured

- Complete assistant messages (after aggregation)
- Final tool call results
- Token usage summaries
- Command execution outcomes
- Error events and retries

## Configuration

### Environment Variables

- `CODEX_OTEL`: Target for traces (file:// URL or OTLP endpoint)
- `CODEX_OTEL_PROTOCOL`: Protocol (binary/json for file, grpc/http for OTLP)
- `CODEX_OTEL_SAMPLE_RATE`: Sampling rate (0.0-1.0, default 1.0)
- `CODEX_OTEL_SERVICE_NAME`: Service name override

### CLI Flags

- `--otel <TARGET>`: Set trace target
- `--otel-protocol <PROTOCOL>`: Set protocol
- `--otel-sample-rate <RATE>`: Set sampling rate
- `--otel-service-name <NAME>`: Set service name

### Default Behavior

When no explicit target is set, traces are automatically written to `~/.codex/traces/codex-{timestamp}-{pid}.log`

### Binary Coverage

All three main Codex binaries now have comprehensive tracing coverage:

1. **`codex` (TUI mode)**: Full conversation tracing with user message spans, LLM request spans, and tool execution spans. Token counts recorded on root session span.

2. **`codex exec` (headless mode)**: Complete conversation tracing with proper span hierarchy. All assistant actions traced under user message spans.

3. **`codex proto` (protocol mode)**: Basic session tracing with git commit and version metadata. Individual conversations traced through the core conversation logic.

## Trace File Format

Traces are written in OTLP format (JSON-encoded) with one span per line. Each span includes:

- Span ID and trace ID for correlation
- Parent-child relationships
- Timestamps and duration
- All configured attributes
- Resource metadata (service info, git commit, etc.)

## Integration with Rollout Files

The tracing system is designed to complement (not replace) the existing rollout recording system:

- **Rollout files**: Complete conversation transcript in application format
- **Trace files**: Performance and debugging information with proper span hierarchy
- Both systems record the same session ID for correlation

## Viewing Traces

### Command Line

```bash
# View with jq for pretty-printing
jq -C . ~/.codex/traces/codex-*.log

# Filter specific spans
jq 'select(.name == "user_message")' ~/.codex/traces/codex-*.log

# View token usage
jq 'select(.attributes.total_tokens != null)' ~/.codex/traces/codex-*.log
```

### OTLP Integration

Export traces to Jaeger, Grafana, or other OpenTelemetry-compatible tools by setting `CODEX_OTEL` to an OTLP endpoint:

```bash
export CODEX_OTEL=http://localhost:4318/v1/traces
export CODEX_OTEL_PROTOCOL=http
codex "analyze this code"
```

## Troubleshooting

### No traces generated

- Check that the `otel` feature is enabled when building
- Verify trace file permissions in `~/.codex/traces/`
- Check for error messages in stderr

### Missing spans

- Ensure you're using the structured conversation flow (not interrupting mid-stream)
- Check that the session completes normally
- Verify sampling rate isn't filtering out spans

### Large trace files

- Adjust `CODEX_OTEL_SAMPLE_RATE` to reduce volume
- Content is automatically truncated to 64KB per field
- Consider rotating old trace files periodically

## Implementation Notes

### Consistent Span Creation

While we attempted to create a shared tracing module to ensure consistency, circular dependency issues prevented this approach. Instead, each binary now uses consistent `tracing::info_span!` calls with standardized field names:

**Core Module (`codex-core`)**:

- Contains helper functions in the `conversation_tracing` module (when `otel` feature is enabled)
- Used for LLM requests, assistant messages, tool calls, and command execution

**Other Binaries (TUI, Exec, CLI)**:

- Use direct `tracing::info_span!` calls with the same field structure
- Ensure consistent span names and attribute keys

**Standardized Field Names**:

- `role`: "user" or "assistant"
- `content`: Message content (truncated to 64KB)
- `message_type`: "user_input" or "assistant_response"
- `tool`: Tool name for function calls
- `cmd`: Command being executed
- `exit_code`, `duration_ms`: Command execution metrics
- `model`, `provider`: LLM request information
- `prompt_tokens`, `completion_tokens`, `total_tokens`: Token usage

This approach maintains consistency while avoiding circular dependencies between crates.
