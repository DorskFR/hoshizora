import { context, trace, type Span, SpanStatusCode } from "@opentelemetry/api";
import { ZoneContextManager } from "@opentelemetry/context-zone";
import { W3CTraceContextPropagator } from "@opentelemetry/core";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { registerInstrumentations } from "@opentelemetry/instrumentation";
import { FetchInstrumentation } from "@opentelemetry/instrumentation-fetch";
import { resourceFromAttributes } from "@opentelemetry/resources";
import { BatchSpanProcessor } from "@opentelemetry/sdk-trace-base";
import { WebTracerProvider } from "@opentelemetry/sdk-trace-web";
import { ATTR_SERVICE_NAME } from "@opentelemetry/semantic-conventions";

import { API_URL, OTLP_TRACES_URL } from "./config.ts";
import { escapeRegExp } from "./regexp.ts";

export function initTelemetry(): void {
  const provider = new WebTracerProvider({
    resource: resourceFromAttributes({
      [ATTR_SERVICE_NAME]: "observatory-web",
      "deployment.environment.name": "demo",
    }),
    spanProcessors: [
      new BatchSpanProcessor(new OTLPTraceExporter({ url: OTLP_TRACES_URL }), {
        scheduledDelayMillis: 1000,
      }),
    ],
  });
  provider.register({
    contextManager: new ZoneContextManager(),
    propagator: new W3CTraceContextPropagator(),
  });
  registerInstrumentations({
    instrumentations: [
      new FetchInstrumentation({
        propagateTraceHeaderCorsUrls: [new RegExp(`^${escapeRegExp(API_URL)}`)],
        clearTimingResources: true,
      }),
    ],
  });
}

const tracer = () => trace.getTracer("observatory-web");

export interface CodeLocation {
  functionName: string;
  filePath: string;
}

/** Wraps a UI handler in a span carrying code.* attributes so hoshizora joins it to the function node. */
export function tracedHandler<T>(
  name: string,
  location: CodeLocation,
  fn: (span: Span) => Promise<T>,
): () => Promise<T> {
  return () =>
    tracer().startActiveSpan(
      name,
      {
        attributes: {
          "code.function.name": location.functionName,
          "code.file.path": location.filePath,
          "ui.handler": true,
        },
      },
      async (span) => {
        try {
          return await context.with(trace.setSpan(context.active(), span), () => fn(span));
        } catch (err) {
          span.recordException(err as Error);
          span.setStatus({ code: SpanStatusCode.ERROR, message: String(err) });
          throw err;
        } finally {
          span.end();
        }
      },
    );
}
