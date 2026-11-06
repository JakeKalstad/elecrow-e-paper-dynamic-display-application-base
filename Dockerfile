FROM rust:1-slim AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config build-essential && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY cli/ cli/
COPY web/ web/
RUN cargo build --locked --release --manifest-path cli/Cargo.toml && cargo build --locked --release --manifest-path web/Cargo.toml
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends fonts-dejavu-core ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/cli/target/release/epaper /usr/local/bin/epaper
COPY --from=build /build/web/target/release/epaper-web /usr/local/bin/epaper-web
WORKDIR /app
ENV EPAPER_BIN=/usr/local/bin/epaper EPAPER_CONFIG=/data/dashboard.json
ENTRYPOINT ["/usr/local/bin/epaper-web"]
