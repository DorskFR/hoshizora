FROM node:24-bookworm-slim AS viewer
WORKDIR /src/viewer
COPY viewer/package.json viewer/package-lock.json ./
RUN npm ci
COPY viewer/ ./
RUN npm run build

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
COPY --from=viewer /src/viewer/dist viewer/dist
RUN cargo build --release -p hoshizora

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl git && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/hoshizora /usr/local/bin/hoshizora
WORKDIR /work
EXPOSE 7317 4318
ENTRYPOINT ["hoshizora"]
CMD ["serve", "--host", "0.0.0.0", "--data-dir", "/work", "--root", "/src", "--reindex"]
