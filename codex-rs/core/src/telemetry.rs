//! Telemetry and structured tracing support for Codex operations.
//!
//! This module provides utilities for creating and managing telemetry spans
//! for various Codex operations like LLM requests, tool calls, and command execution.

// Tracing imports are handled within the feature-gated modules

/// Maximum content size for telemetry attributes to avoid overwhelming trace storage.
const OTEL_CONTENT_LIMIT: usize = 64 * 1024;

/// Truncate content to a reasonable size for telemetry attributes.
pub fn truncate_content(s: &str) -> String {
    if s.len() > OTEL_CONTENT_LIMIT {
        s.chars().take(OTEL_CONTENT_LIMIT).collect()
    } else {
        s.to_string()
    }
}

/// An abstraction for propagating trace context between components.
///
/// This struct provides a simple way to capture the current tracing context
/// and later create child spans from it, hiding the complexity of OpenTelemetry
/// context propagation from the business logic.
#[derive(Clone, Debug, Default)]
pub struct TraceContext {
    /// The serialized trace context information.
    /// When `None`, no context propagation will occur.
    inner: Option<std::collections::HashMap<String, String>>,
}

impl TraceContext {
    /// Create a new empty trace context.
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Create a trace context from an existing context map.
    pub fn from_context_map(context_map: Option<std::collections::HashMap<String, String>>) -> Self {
        Self { inner: context_map }
    }

    /// Capture the current trace context.
    ///
    /// In OpenTelemetry-enabled builds, this captures the current context for later propagation.
    /// In builds without OpenTelemetry, this returns an empty context.
    #[cfg(feature = "otel")]
    pub fn capture_current() -> Self {
        use std::collections::HashMap;
        
        let current_context = opentelemetry::Context::current();
        let mut carrier = HashMap::new();
        opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&current_context, &mut carrier);
        });
        
        Self {
            inner: if carrier.is_empty() { None } else { Some(carrier) },
        }
    }
    
    /// Capture the current trace context (no-op version for non-OpenTelemetry builds).
    #[cfg(not(feature = "otel"))]
    pub fn capture_current() -> Self {
        Self { inner: None }
    }
    
    /// Get the inner context map for serialization.
    pub fn into_inner(self) -> Option<std::collections::HashMap<String, String>> {
        self.inner
    }
    
    /// Create a span with this context as parent.
    #[cfg(feature = "otel")]
    pub fn create_span(&self, span_name: &str) -> tracing::Span {
        match &self.inner {
            Some(context_map) => {
                let parent_context = opentelemetry::global::get_text_map_propagator(|propagator| {
                    propagator.extract(context_map)
                });
                
                // Temporarily set the extracted context as current to create child spans
                let _guard = parent_context.attach();
                
                // Create a span as a child of the attached context
                match span_name {
                    "user_message" => tracing::info_span!("user_message"),
                    "llm_request" => tracing::info_span!("llm_request"),
                    "assistant_msg" => tracing::info_span!("assistant_msg"),
                    "tool_call" => tracing::info_span!("tool_call"),
                    "exec_cmd" => tracing::info_span!("exec_cmd"),
                    "function_call_output" => tracing::info_span!("function_call_output"),
                    _ => tracing::info_span!("span"),
                }
            }
            None => {
                // Create a span without parent context
                match span_name {
                    "user_message" => tracing::info_span!("user_message"),
                    "llm_request" => tracing::info_span!("llm_request"),
                    "assistant_msg" => tracing::info_span!("assistant_msg"),
                    "tool_call" => tracing::info_span!("tool_call"),
                    "exec_cmd" => tracing::info_span!("exec_cmd"),
                    "function_call_output" => tracing::info_span!("function_call_output"),
                    _ => tracing::info_span!("span"),
                }
            }
        }
    }
    
    /// Create a span with this context as parent (no-op version for non-OpenTelemetry builds).
    #[cfg(not(feature = "otel"))]
    pub fn create_span(&self, span_name: &str) -> tracing::Span {
        match span_name {
            "user_message" => tracing::info_span!("user_message"),
            "llm_request" => tracing::info_span!("llm_request"),
            "assistant_msg" => tracing::info_span!("assistant_msg"),
            "tool_call" => tracing::info_span!("tool_call"),
            "exec_cmd" => tracing::info_span!("exec_cmd"),
            "function_call_output" => tracing::info_span!("function_call_output"),
            _ => tracing::info_span!("span"),
        }
    }
    
    /// Create a user message span with this context as parent.
    pub fn create_user_message_span(&self, content: &str) -> tracing::Span {
        if content.is_empty() {
            let span = self.create_span("user_message");
            span.record("content", "non-text-input");
            return span;
        }
        
        #[cfg(feature = "otel")]
        {
            if let Some(context_map) = &self.inner {
                let parent_context = opentelemetry::global::get_text_map_propagator(|propagator| {
                    propagator.extract(context_map)
                });
                
                // Temporarily set the extracted context as current to create child spans
                let _guard = parent_context.attach();
                
                // Create span as a child of the attached context
                return tracing::info_span!(
                    "user_message",
                    role = "user",
                    content = truncate_content(content),
                    message_type = "user_input"
                );
            }
        }
        
        // Fall back to regular span creation
        conversation_tracing::create_user_message_span(content)
    }
}

/// Structured tracing support for conversation events.
///
/// This module provides span creation functions that are used when the `otel` feature
/// is enabled to create properly structured OpenTelemetry traces.
#[cfg(feature = "otel")]
pub mod conversation_tracing {
    use super::*;
    use tracing::{info_span, Span};
    
    /// Create a user message span within the current context
    pub fn create_user_message_span(content: &str) -> Span {
        info_span!(
            "user_message",
            role = "user",
            content = truncate_content(content),
            message_type = "user_input"
        )
    }
    
    /// Create an LLM request span for assistant interactions
    pub fn create_llm_request_span(model: &str, provider: &str) -> Span {
        info_span!(
            "llm_request",
            model = model,
            provider = provider,
            prompt_tokens = tracing::field::Empty,
            completion_tokens = tracing::field::Empty,
            total_tokens = tracing::field::Empty,
            cached_tokens = tracing::field::Empty,
            reasoning_tokens = tracing::field::Empty,
            retries = tracing::field::Empty
        )
    }
    
    /// Create a span for assistant messages
    pub fn create_assistant_message_span() -> Span {
        info_span!(
            "assistant_msg",
            role = "assistant",
            content = tracing::field::Empty,
            message_type = "assistant_response"
        )
    }
    
    /// Create a span for tool calls
    pub fn create_tool_call_span(tool_name: &str, args: &str) -> Span {
        info_span!(
            "tool_call",
            tool = tool_name,
            args = truncate_content(args),
            call_type = "function_call"
        )
    }
    
    /// Create a span for command execution
    pub fn create_exec_cmd_span(cmd: &str) -> Span {
        info_span!(
            "exec_cmd",
            cmd = cmd,
            exit_code = tracing::field::Empty,
            duration_ms = tracing::field::Empty,
            stdout_size = tracing::field::Empty,
            stderr_size = tracing::field::Empty,
            status = tracing::field::Empty,
            working_directory = tracing::field::Empty
        )
    }
    
    /// Create a span for function call outputs
    pub fn create_function_call_output_span(call_id: &str) -> Span {
        info_span!(
            "function_call_output",
            call_id = call_id,
            success = tracing::field::Empty,
            content_size = tracing::field::Empty,
            call_type = "function_output",
            content = tracing::field::Empty
        )
    }
    
    /// Record token usage in the current span
    pub fn record_token_usage(
        input_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
        cached_tokens: Option<u64>,
        reasoning_tokens: Option<u64>,
    ) {
        let current_span = Span::current();
        current_span.record("prompt_tokens", input_tokens);
        current_span.record("completion_tokens", output_tokens);
        current_span.record("total_tokens", total_tokens);
        if let Some(cached) = cached_tokens {
            current_span.record("cached_tokens", cached);
        }
        if let Some(reasoning) = reasoning_tokens {
            current_span.record("reasoning_tokens", reasoning);
        }
    }
}

/// Re-export the conversation_tracing module when otel feature is disabled
/// to provide consistent API.
#[cfg(not(feature = "otel"))]
pub mod conversation_tracing {
    
    /// Create a no-op user message span when telemetry is disabled
    pub fn create_user_message_span(_content: &str) -> tracing::Span {
        tracing::Span::none()
    }
    
    /// Create a no-op LLM request span when telemetry is disabled
    pub fn create_llm_request_span(_model: &str, _provider: &str) -> tracing::Span {
        tracing::Span::none()
    }
    
    /// Create a no-op assistant message span when telemetry is disabled
    pub fn create_assistant_message_span() -> tracing::Span {
        tracing::Span::none()
    }
    
    /// Create a no-op tool call span when telemetry is disabled
    pub fn create_tool_call_span(_tool_name: &str, _args: &str) -> tracing::Span {
        tracing::Span::none()
    }
    
    /// Create a no-op exec command span when telemetry is disabled
    pub fn create_exec_cmd_span(_cmd: &str) -> tracing::Span {
        tracing::Span::none()
    }
    
    /// Create a no-op function call output span when telemetry is disabled
    pub fn create_function_call_output_span(_call_id: &str) -> tracing::Span {
        tracing::Span::none()
    }
    
    /// No-op token usage recording when telemetry is disabled
    pub fn record_token_usage(
        _input_tokens: u64,
        _output_tokens: u64,
        _total_tokens: u64,
        _cached_tokens: Option<u64>,
        _reasoning_tokens: Option<u64>,
    ) {
        // No-op when telemetry is disabled
    }
} 