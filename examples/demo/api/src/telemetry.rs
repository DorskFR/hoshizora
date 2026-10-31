use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue};
use opentelemetry_http::{HeaderExtractor, HeaderInjector};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub fn init(service_name: &str) -> anyhow::Result<SdkTracerProvider> {
    global::set_text_map_propagator(TraceContextPropagator::new());

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:7317".into());
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(format!("{}/v1/traces", endpoint.trim_end_matches('/')))
        .build()?;

    let resource = Resource::builder()
        .with_service_name(service_name.to_string())
        .with_attribute(KeyValue::new("deployment.environment.name", "demo"))
        .build();
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();
    global::set_tracer_provider(provider.clone());

    let tracer = provider.tracer(service_name.to_string());
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .init();
    Ok(provider)
}

pub async fn http_server_span(req: Request, next: Next) -> Response {
    let parent = global::get_text_map_propagator(|p| p.extract(&HeaderExtractor(req.headers())));
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| req.uri().path().to_owned());
    let method = req.method().as_str().to_owned();
    let span = tracing::info_span!(
        "http.server",
        otel.name = format!("{method} {route}"),
        otel.kind = "server",
        http.request.method = %method,
        http.route = %route,
        url.path = %req.uri().path(),
        http.response.status_code = tracing::field::Empty,
    );
    span.set_parent(parent);
    async move {
        let res = next.run(req).await;
        tracing::Span::current().record("http.response.status_code", res.status().as_u16());
        res
    }
    .instrument(span)
    .await
}

pub fn inject_traceparent(headers: &mut http::HeaderMap) {
    let cx = tracing::Span::current().context();
    global::get_text_map_propagator(|p| p.inject_context(&cx, &mut HeaderInjector(headers)));
}
