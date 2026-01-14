FROM rust:1.83-slim-bookworm as builder
WORKDIR /usr/src/app
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY migration/Cargo.toml migration/
COPY migration/src migration/src/
mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src
COPY . .
RUN cargo build --release

FROM gcr.io/distroless/cc-debian12
WORKDIR /app
COPY --from=builder /usr/src/app/target/release/pulse_framework /app/pulse_framework
ENV RUST_LOG=info
EXPOSE 8080
CMD ["./pulse_framework"]
