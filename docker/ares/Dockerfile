# ares (the reference N64 emulator) for headless test runs:
#   cargo xtask emulators setup ares     # docker build -t trino-ares docker/ares
#   TRINO_ARES=docker cargo xtask test n64
# The ROM directory is mounted at /rom; ares runs under Xvfb with Mesa's
# software Vulkan (lavapipe) for parallel-RDP, audio off. ares ships no
# Linux release binary, so it is built from the pinned tag (N64 core only).
#
# ARES_BUILD_LOCAL=OFF: ares defaults to -march=native, so an image built on
# one CI runner died with "Illegal instruction" on a runner with an older CPU
# (the image is cached across runs). OFF builds for x86-64-v2, which every
# x86-64 runner supports.

FROM ubuntu:24.04 AS build
ARG ARES_TAG=v148
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
        build-essential cmake ninja-build git pkg-config ca-certificates \
        libgtk-3-dev libsdl2-dev libgl-dev libgl1-mesa-dev libao-dev \
        libopenal-dev libpulse-dev libasound2-dev libxrandr-dev libudev-dev \
        libxi-dev \
    && rm -rf /var/lib/apt/lists/*
RUN git clone -q --depth 1 --branch "$ARES_TAG" https://github.com/ares-emulator/ares.git /src \
    && cmake -S /src -B /src/build -G Ninja -DCMAKE_BUILD_TYPE=Release -DARES_CORES=n64 \
        -DARES_BUILD_LOCAL=OFF \
    && cmake --build /src/build -j"$(nproc)" \
    && cmake --install /src/build --prefix /opt/ares

FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
        xvfb xauth libgtk-3-0t64 libsdl2-2.0-0 libao4 libopenal1 libpulse0 \
        libasound2t64 libxrandr2 libudev1 libxi6 libopengl0 libglx0 libgl1 \
        libvulkan1 mesa-vulkan-drivers \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /opt/ares /opt/ares
ENV PATH=/opt/ares/bin:$PATH
# ISViewer output goes to stdout; xtask reads it through `docker run`.
ENTRYPOINT ["xvfb-run", "-a", "-s", "-screen 0 1280x720x24", "ares"]
