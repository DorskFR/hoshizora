//! OTLP/HTTP trace export decoding (protobuf and JSON) into `RuntimeSpan`s.

use std::collections::BTreeMap;
use std::io::Read;

use anyhow::{Context, bail};
use hoshizora_model::{RuntimeSpan, SpanKind};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::trace::v1::{Span, span, status};
use prost::Message;

pub const CONTENT_TYPE_PROTOBUF: &str = "application/x-protobuf";
pub const CONTENT_TYPE_JSON: &str = "application/json";

/// Decodes an OTLP/HTTP export body according to its `Content-Type` and `Content-Encoding`.
pub fn decode(
    content_type: &str,
    content_encoding: Option<&str>,
    body: &[u8],
) -> anyhow::Result<Vec<RuntimeSpan>> {
    let body = match content_encoding.map(str::trim).map(str::to_ascii_lowercase) {
        Some(enc) if enc == "gzip" || enc == "x-gzip" => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(body)
                .read_to_end(&mut out)
                .context("gzip body")?;
            out
        }
        Some(enc) if enc.is_empty() || enc == "identity" => body.to_vec(),
        Some(enc) => bail!("unsupported Content-Encoding: {enc}"),
        None => body.to_vec(),
    };
    let request = decode_request(content_type, &body)?;
    Ok(normalise(request))
}

pub fn decode_request(
    content_type: &str,
    body: &[u8],
) -> anyhow::Result<ExportTraceServiceRequest> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        CONTENT_TYPE_PROTOBUF | "application/protobuf" | "" => {
            ExportTraceServiceRequest::decode(body).context("protobuf body")
        }
        CONTENT_TYPE_JSON => serde_json::from_slice(body).context("json body"),
        other => bail!("unsupported Content-Type: {other}"),
    }
}

pub fn normalise(request: ExportTraceServiceRequest) -> Vec<RuntimeSpan> {
    let mut out = Vec::new();
    for resource_spans in request.resource_spans {
        let resource_attrs = resource_spans
            .resource
            .map(|r| flatten(&r.attributes))
            .unwrap_or_default();
        let service = resource_attrs
            .get("service.name")
            .cloned()
            .unwrap_or_else(|| "unknown_service".to_string());
        let version = resource_attrs.get("service.version");
        for scope_spans in resource_spans.scope_spans {
            for s in scope_spans.spans {
                let mut span = normalise_span(s, &service);
                if let Some(v) = version {
                    span.attrs
                        .entry("service.version".into())
                        .or_insert_with(|| v.clone());
                }
                out.push(span);
            }
        }
    }
    out
}

fn normalise_span(s: Span, service: &str) -> RuntimeSpan {
    let attrs = flatten(&s.attributes);
    let kind = match span::SpanKind::try_from(s.kind).unwrap_or(span::SpanKind::Unspecified) {
        span::SpanKind::Server => SpanKind::Server,
        span::SpanKind::Client => SpanKind::Client,
        span::SpanKind::Producer => SpanKind::Producer,
        span::SpanKind::Consumer => SpanKind::Consumer,
        span::SpanKind::Internal | span::SpanKind::Unspecified => SpanKind::Internal,
    };
    let status_error = s
        .status
        .as_ref()
        .is_some_and(|st| st.code == status::StatusCode::Error as i32);
    let http_5xx = ["http.response.status_code", "http.status_code"]
        .iter()
        .filter_map(|k| attrs.get(*k))
        .any(|v| v.parse::<u16>().is_ok_and(|c| c >= 500));
    RuntimeSpan {
        trace_id: hex(&s.trace_id),
        span_id: hex(&s.span_id),
        parent_span_id: (!s.parent_span_id.is_empty()).then(|| hex(&s.parent_span_id)),
        service: service.to_string(),
        name: s.name,
        kind,
        start_unix_nanos: s.start_time_unix_nano,
        end_unix_nanos: s.end_time_unix_nano,
        error: status_error || http_5xx,
        attrs,
        node: None,
        join: None,
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn flatten(attrs: &[KeyValue]) -> BTreeMap<String, String> {
    attrs
        .iter()
        .filter_map(|kv| {
            kv.value
                .as_ref()
                .and_then(any_value_to_string)
                .map(|v| (kv.key.clone(), v))
        })
        .collect()
}

fn any_value_to_string(v: &AnyValue) -> Option<String> {
    Some(match v.value.as_ref()? {
        any_value::Value::StringValue(s) => s.clone(),
        any_value::Value::BoolValue(b) => b.to_string(),
        any_value::Value::IntValue(i) => i.to_string(),
        any_value::Value::DoubleValue(d) => d.to_string(),
        any_value::Value::BytesValue(b) => hex(b),
        any_value::Value::StringValueStrindex(i) => format!("strindex:{i}"),
        any_value::Value::ArrayValue(a) => {
            let items: Vec<serde_json::Value> = a
                .values
                .iter()
                .map(|x| {
                    any_value_to_string(x)
                        .map_or(serde_json::Value::Null, serde_json::Value::String)
                })
                .collect();
            serde_json::Value::Array(items).to_string()
        }
        any_value::Value::KvlistValue(kv) => {
            let map: serde_json::Map<String, serde_json::Value> = flatten(&kv.values)
                .into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect();
            serde_json::Value::Object(map).to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Status};

    fn kv(key: &str, value: any_value::Value) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue { value: Some(value) }),
            ..Default::default()
        }
    }

    pub(crate) fn sample_request() -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![kv(
                        "service.name",
                        any_value::Value::StringValue("cctui-server".into()),
                    )],
                    ..Default::default()
                }),
                scope_spans: vec![ScopeSpans {
                    scope: Some(InstrumentationScope::default()),
                    spans: vec![
                        Span {
                            trace_id: vec![1; 16],
                            span_id: vec![2; 8],
                            name: "POST /api/sessions".into(),
                            kind: span::SpanKind::Server as i32,
                            start_time_unix_nano: 1_000,
                            end_time_unix_nano: 5_000,
                            attributes: vec![
                                kv(
                                    "http.route",
                                    any_value::Value::StringValue("/api/sessions".into()),
                                ),
                                kv(
                                    "http.request.method",
                                    any_value::Value::StringValue("POST".into()),
                                ),
                                kv("http.response.status_code", any_value::Value::IntValue(201)),
                                kv("retry", any_value::Value::BoolValue(true)),
                                kv("ratio", any_value::Value::DoubleValue(0.5)),
                            ],
                            status: Some(Status {
                                code: status::StatusCode::Ok as i32,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        Span {
                            trace_id: vec![1; 16],
                            span_id: vec![3; 8],
                            parent_span_id: vec![2; 8],
                            name: "create".into(),
                            kind: span::SpanKind::Internal as i32,
                            start_time_unix_nano: 2_000,
                            end_time_unix_nano: 4_000,
                            attributes: vec![
                                kv(
                                    "code.namespace",
                                    any_value::Value::StringValue(
                                        "cctui_server::api::sessions".into(),
                                    ),
                                ),
                                kv(
                                    "code.function.name",
                                    any_value::Value::StringValue("create".into()),
                                ),
                            ],
                            status: Some(Status {
                                code: status::StatusCode::Error as i32,
                                message: "boom".into(),
                            }),
                            ..Default::default()
                        },
                    ],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
    }

    #[test]
    fn protobuf_round_trip() {
        let body = sample_request().encode_to_vec();
        let spans = decode("application/x-protobuf", None, &body).unwrap();
        assert_eq!(spans.len(), 2);
        let root = &spans[0];
        assert_eq!(root.trace_id, "01".repeat(16));
        assert_eq!(root.span_id, "02".repeat(8));
        assert_eq!(root.parent_span_id, None);
        assert_eq!(root.service, "cctui-server");
        assert_eq!(root.kind, SpanKind::Server);
        assert_eq!(root.duration_nanos(), 4_000);
        assert!(!root.error);
        assert_eq!(root.attrs["http.response.status_code"], "201");
        assert_eq!(root.attrs["retry"], "true");
        assert_eq!(root.attrs["ratio"], "0.5");
        let child = &spans[1];
        assert_eq!(
            child.parent_span_id.as_deref(),
            Some("02".repeat(8).as_str())
        );
        assert!(child.error);
        assert_eq!(child.attrs["code.namespace"], "cctui_server::api::sessions");
    }

    #[test]
    fn gzip_protobuf() {
        use std::io::Write;
        let body = sample_request().encode_to_vec();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&body).unwrap();
        let gz = enc.finish().unwrap();
        let spans = decode("application/x-protobuf", Some("gzip"), &gz).unwrap();
        assert_eq!(spans.len(), 2);
        assert!(decode("application/x-protobuf", Some("br"), &gz).is_err());
    }

    #[test]
    fn json_round_trip_of_proto_types() {
        let json = serde_json::to_vec(&sample_request()).unwrap();
        let spans = decode("application/json; charset=utf-8", None, &json).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].trace_id, "01".repeat(16));
        assert_eq!(spans[0].start_unix_nanos, 1_000);
    }

    #[test]
    fn real_otlp_json_sample() {
        let json = r#"{
  "resourceSpans": [{
    "resource": {
      "attributes": [
        {"key": "service.name", "value": {"stringValue": "webui"}},
        {"key": "service.version", "value": {"stringValue": "0.29.0-beta.1"}},
        {"key": "telemetry.sdk.language", "value": {"stringValue": "webjs"}},
        {"key": "process.pid", "value": {"intValue": "4242"}}
      ]
    },
    "scopeSpans": [{
      "scope": {"name": "@opentelemetry/instrumentation-fetch", "version": "0.50.0"},
      "spans": [{
        "traceId": "5b8aa5a2d2c872e8321cf37308d69df2",
        "spanId": "051581bf3cb55c13",
        "parentSpanId": "",
        "name": "HTTP POST",
        "kind": 3,
        "startTimeUnixNano": "1718100000000000000",
        "endTimeUnixNano": "1718100000250000000",
        "attributes": [
          {"key": "http.method", "value": {"stringValue": "POST"}},
          {"key": "http.url", "value": {"stringValue": "http://localhost:7317/api/sessions"}},
          {"key": "http.status_code", "value": {"intValue": 502}},
          {"key": "http.response_content_length", "value": {"intValue": "12"}},
          {"key": "tags", "value": {"arrayValue": {"values": [{"stringValue": "a"}, {"stringValue": "b"}]}}}
        ],
        "droppedAttributesCount": 0,
        "events": [],
        "links": [],
        "status": {"code": 2, "message": "Bad Gateway"}
      }]
    }]
  }]
}"#;
        let spans = decode("application/json", None, json.as_bytes()).unwrap();
        assert_eq!(spans.len(), 1);
        let s = &spans[0];
        assert_eq!(s.trace_id, "5b8aa5a2d2c872e8321cf37308d69df2");
        assert_eq!(s.span_id, "051581bf3cb55c13");
        assert_eq!(s.parent_span_id, None);
        assert_eq!(s.service, "webui");
        assert_eq!(s.attrs["service.version"], "0.29.0-beta.1");
        assert_eq!(s.kind, SpanKind::Client);
        assert_eq!(s.start_unix_nanos, 1_718_100_000_000_000_000);
        assert_eq!(s.duration_nanos(), 250_000_000);
        assert!(s.error);
        assert_eq!(s.attrs["http.status_code"], "502");
        assert_eq!(s.attrs["http.response_content_length"], "12");
        assert_eq!(s.attrs["tags"], r#"["a","b"]"#);
    }

    #[test]
    fn rejects_unknown_content_type() {
        assert!(decode("text/plain", None, b"hello").is_err());
    }
}
