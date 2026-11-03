# The lobby server online players meet at (issue #117, ADR 0041): rooms by
# code, the settings fingerprint check, spectators, the relay for peers whose
# NATs can't be punched (two symmetric NATs, a symmetric one facing a
# port-restricted one), and the WebSocket signalling of the Web's WebRTC.
# It is `cargo xtask net lobby`, packaged. Nothing here is run publicly by
# the project: deploy it where your players can reach UDP 7400 and TCP 7401.
#
#   docker build -f docker/lobby/Dockerfile -t trino-lobby .
#   docker run --rm -p 7400:7400/udp -p 7401:7401 trino-lobby
#
# Players: TRINO_NET=lobby:<code>@<server>:7400 (PC),
# ?net=<code>&lobby=ws://<server>:7401 (Web); spectators TRINO_NET=watch:...
FROM rust:1-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libasound2-dev libudev-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# rust-toolchain.toml picks the toolchain; only the lobby's binary is kept.
RUN cargo build --release -p xtask

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends libasound2 libudev1 \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/xtask /usr/local/bin/trino-xtask
# xtask resolves the checkout it was built from at start: the folders, empty.
RUN mkdir -p /src/xtask
WORKDIR /src
EXPOSE 7400/udp 7401/tcp
ENTRYPOINT ["trino-xtask", "net", "lobby", "--udp", "7400", "--ws", "7401"]
