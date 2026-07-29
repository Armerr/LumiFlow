# Stage 1: Frontend build
FROM node:22-alpine AS frontend
WORKDIR /build
COPY web/package.json web/package-lock.json* ./
RUN npm ci 2>/dev/null || npm install
COPY web/ ./
RUN npm run build

# Stage 2: Rust build (glibc; ort rc.13 requires the GNU C++ ABI shipped by Debian Trixie)
FROM rust:1.88-trixie AS backend
ARG LUMIFLOW_CARGO_FEATURES=""
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs
# Pre-fetch deps (dummy build)
RUN cargo build --release ${LUMIFLOW_CARGO_FEATURES:+--features "$LUMIFLOW_CARGO_FEATURES"} 2>/dev/null || true
RUN rm -rf src target/release/lumiflow target/release/deps/lumiflow-* target/release/.fingerprint/lumiflow-*
COPY src/ ./src/
COPY --from=frontend /build/dist ./web/dist/
RUN cargo build --release --locked ${LUMIFLOW_CARGO_FEATURES:+--features "$LUMIFLOW_CARGO_FEATURES"}
RUN strip target/release/lumiflow

# Stage 3: Runtime
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates libstdc++6 tzdata \
    && rm -rf /var/lib/apt/lists/*
COPY --from=backend /build/target/release/lumiflow /usr/local/bin/lumiflow
EXPOSE 4320
ENV LUMIFLOW_PORT=4320
ENV LUMIFLOW_BIND_ADDRESS=0.0.0.0
ENTRYPOINT ["lumiflow"]
