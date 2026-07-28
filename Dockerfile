# Stage 1: Frontend build
FROM node:22-alpine AS frontend
WORKDIR /build
COPY web/package.json web/package-lock.json* ./
RUN npm ci 2>/dev/null || npm install
COPY web/ ./
RUN npm run build

# Stage 2: Rust build
FROM rust:1.88-alpine AS backend
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs
# Pre-fetch deps (dummy build)
RUN cargo build --release 2>/dev/null || true
RUN rm -rf src target/release/lumiflow target/release/deps/lumiflow-* target/release/.fingerprint/lumiflow-*
COPY src/ ./src/
COPY --from=frontend /build/dist ./web/dist/
RUN cargo build --release --locked
RUN strip target/release/lumiflow

# Stage 3: Runtime
FROM alpine:3.21
RUN apk add --no-cache ca-certificates tzdata
COPY --from=backend /build/target/release/lumiflow /usr/local/bin/lumiflow
EXPOSE 4320
ENV LUMIFLOW_PORT=4320
ENV LUMIFLOW_BIND_ADDRESS=0.0.0.0
ENTRYPOINT ["lumiflow"]
