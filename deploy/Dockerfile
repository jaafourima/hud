# Builds hud-server from the repository root:  docker build -f deploy/Dockerfile -t hud-server .
FROM rust:1.89-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p hud-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home hud
COPY --from=build /src/target/release/hud-server /usr/local/bin/hud-server
USER hud
EXPOSE 8080
CMD ["hud-server"]
