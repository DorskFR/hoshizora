export const API_URL: string = (import.meta.env.VITE_API_URL as string | undefined) ?? "http://localhost:8081";
export const OTLP_TRACES_URL: string =
  (import.meta.env.VITE_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT as string | undefined) ??
  "http://localhost:7317/v1/traces";
