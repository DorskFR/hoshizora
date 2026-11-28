FROM node:24-alpine AS viewer
WORKDIR /src/viewer
COPY viewer/package.json viewer/package-lock.json ./
RUN npm ci
COPY viewer/ ./
RUN npm run build

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
COPY --from=viewer /src/viewer/dist viewer/dist
RUN cargo build --release -p hoshizora

FROM alpine:3
RUN apk upgrade --no-cache && apk add --no-cache ca-certificates git
COPY --from=build /src/target/release/hoshizora /usr/local/bin/hoshizora
WORKDIR /work
EXPOSE 7317 4318
ENTRYPOINT ["hoshizora"]
CMD ["serve", "--host", "0.0.0.0", "--data-dir", "/work", "--root", "/src", "--reindex"]
