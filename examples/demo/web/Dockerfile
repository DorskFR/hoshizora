FROM node:22-alpine AS build
WORKDIR /src
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
ARG VITE_API_URL=http://localhost:8081
ARG VITE_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://localhost:7317/v1/traces
RUN npm run build

FROM nginx:alpine
COPY --from=build /src/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
