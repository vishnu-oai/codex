use tracing_subscriber::fmt;

#[cfg(feature = "otel")]
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Registry};
#[cfg(feature = "otel")]
use opentelemetry::trace::TracerProvider;
#[cfg(feature = "otel")]
use opentelemetry::KeyValue;
#[cfg(feature = "otel")]
use opentelemetry_sdk::{trace as sdktrace, Resource};
#[cfg(feature = "otel")]
use opentelemetry_stdout;
#[cfg(feature = "otel")]
use opentelemetry_otlp::WithExportConfig;
#[cfg(feature = "otel")]
use tracing_opentelemetry;

#[cfg(feature = "otel")]
use opentelemetry_sdk::trace::{SpanExporter, SpanData};
#[cfg(feature = "otel")]
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};

#[cfg(feature = "otel")]
use std::{
    fs::{create_dir_all, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::SystemTime,
};

/// Configuration for initializing OpenTelemetry tracing.
#[derive(Default)]
pub struct OtelConfig {
    pub target: Option<String>,
    pub protocol: Option<String>,
    pub sample_rate: Option<f64>,
    pub service_name: Option<String>,
}

/// Initialize tracing subscriber, no‑op when otel feature is disabled.
#[cfg(not(feature = "otel"))]
pub fn init_telemetry(_config: OtelConfig) {
    let _ = fmt().try_init();
}

#[cfg(feature = "otel")]
#[derive(Debug)]
struct FileSpanExporter {
    file: Arc<Mutex<std::fs::File>>, 
}

#[cfg(feature = "otel")]
impl FileSpanExporter {
    fn new(path: PathBuf) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
        })
    }
}

#[cfg(feature = "otel")]
impl SpanExporter for FileSpanExporter {
    fn export(&self, batch: Vec<SpanData>) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        let file = self.file.clone();
        async move {
            let mut buf = String::new();
            for span in batch {
                // Convert SpanData to OTLP protobuf format, then to JSON
                let otlp_span = opentelemetry_proto::tonic::trace::v1::Span::from(span);
                match serde_json::to_string(&otlp_span) {
                    Ok(json) => {
                        buf.push_str(&json);
                        buf.push('\n');
                    }
                    Err(e) => {
                        eprintln!("Failed to serialize span to JSON: {}", e);
                        // Fallback to debug format if JSON serialization fails
                        buf.push_str(&format!("{:?}\n", otlp_span));
                    }
                }
            }

            match file.lock() {
                Ok(mut f) => {
                    if let Err(e) = f.write_all(buf.as_bytes()) {
                        return Err(OTelSdkError::InternalFailure(e.to_string()));
                    }
                }
                Err(e) => return Err(OTelSdkError::InternalFailure(e.to_string())),
            }

            Ok(())
        }
    }
}

/// Generate a default trace file path in CODEX_HOME/traces/
#[cfg(feature = "otel")]
fn generate_default_trace_file() -> Option<String> {
    // Resolve CODEX_HOME (same logic as config loading)
    let codex_home = std::env::var("CODEX_HOME").ok()
        .map(PathBuf::from)
        .or_else(|| {
            dirs::home_dir().map(|home| home.join(".codex"))
        })?;
    
    // Create traces directory if it doesn't exist
    let traces_dir = codex_home.join("traces");
    if let Err(e) = create_dir_all(&traces_dir) {
        eprintln!("Failed to create traces directory: {e}");
        return None;
    }
    
    // Generate unique filename based on timestamp and process ID
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let pid = std::process::id();
    let filename = format!("codex-{}-{}.log", timestamp, pid);
    
    let trace_file = traces_dir.join(filename);
    Some(format!("file://{}", trace_file.display()))
}

/// Initialize tracing subscriber with OpenTelemetry exporter.
#[cfg(feature = "otel")]
pub fn init_telemetry(config: OtelConfig) {
    let explicit_target = config.target.is_some() || std::env::var("CODEX_OTEL").is_ok();
    
    let target = config
        .target
        .or_else(|| std::env::var("CODEX_OTEL").ok())
        .or_else(|| generate_default_trace_file());

    // If no telemetry target is specified, just use basic formatting.
    let Some(target) = target else {
        let _ = fmt().try_init();
        return;
    };
    
    // Print the trace file location for user awareness
    if target.starts_with("file://") && !explicit_target {
        let path = target.trim_start_matches("file://");
        eprintln!("📊 Tracing enabled: {}", path);
    }

    let protocol = config
        .protocol
        .or_else(|| std::env::var("CODEX_OTEL_PROTOCOL").ok())
        .unwrap_or_else(|| "grpc".to_string());

    let service_name = config
        .service_name
        .or_else(|| std::env::var("CODEX_OTEL_SERVICE_NAME").ok())
        .unwrap_or_else(|| "codex-cli".to_string());

    let sample_rate = config
        .sample_rate
        .or_else(|| std::env::var("CODEX_OTEL_SAMPLE_RATE").ok().and_then(|v| v.parse().ok()))
        .unwrap_or(1.0);

    static VERSION: &str = env!("CARGO_PKG_VERSION");
    static REPO: &str = env!("CARGO_PKG_REPOSITORY");

    let resource = Resource::builder_empty()
        .with_attributes([
            KeyValue::new("service.name", service_name),
            KeyValue::new("service.version", VERSION),
            KeyValue::new("git.repository_url", REPO),
        ])
        .build();

    let fmt_layer = fmt::layer();

    if target.starts_with("file://") {
        // Path is everything after scheme.
        let path = target.trim_start_matches("file://");
        match FileSpanExporter::new(PathBuf::from(path)) {
            Ok(exporter) => {
                let provider = sdktrace::SdkTracerProvider::builder()
                    .with_resource(resource)
                    .with_sampler(sdktrace::Sampler::TraceIdRatioBased(sample_rate))
                    .with_simple_exporter(exporter)
                    .build();

                opentelemetry::global::set_tracer_provider(provider.clone());
                let tracer = provider.tracer("codex-cli");
                let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

                Registry::default()
                    .with(fmt_layer)
                    .with(otel_layer)
                    .init();
            }
            Err(e) => {
                eprintln!("Failed to create file exporter: {e}");
                let _ = fmt().try_init();
            }
        }
    } else if target == "stdout" {
        let exporter = opentelemetry_stdout::SpanExporter::default();
        let provider = sdktrace::SdkTracerProvider::builder()
            .with_resource(resource)
            .with_sampler(sdktrace::Sampler::TraceIdRatioBased(sample_rate))
            .with_simple_exporter(exporter)
            .build();

        opentelemetry::global::set_tracer_provider(provider.clone());
        let tracer = provider.tracer("codex-cli");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        Registry::default()
            .with(fmt_layer)
            .with(otel_layer)
            .init();
    } else {
        let exporter_result = if protocol == "http" {
            opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_endpoint(&target)
                .build()
        } else {
            opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(&target)
                .build()
        };

        match exporter_result {
            Ok(exporter) => {
                let provider = sdktrace::SdkTracerProvider::builder()
                    .with_resource(resource)
                    .with_sampler(sdktrace::Sampler::TraceIdRatioBased(sample_rate))
                    .with_batch_exporter(exporter)
                    .build();

                opentelemetry::global::set_tracer_provider(provider.clone());
                let tracer = provider.tracer("codex-cli");
                let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

                Registry::default()
                    .with(fmt_layer)
                    .with(otel_layer)
                    .init();
            }
            Err(e) => {
                eprintln!("Failed to create OTLP exporter: {e}");
                let _ = fmt().try_init();
            }
        }
    }
}
