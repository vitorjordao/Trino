# Trino's N64 build image: libdragon's official toolchain package (GCC for
# mips64-elf, from libdragon's GitHub releases) plus libdragon itself
# (library + headers + asset/ROM tools) at a pinned commit.
#
# Rebuild (xtask does this automatically when missing):
#   docker build -t trino-n64 docker/n64
#
# Pins (never a moving tag — part of the reproducible-build story):
# - LIBDRAGON_COMMIT: bumping libdragon = changing it.
# - TOOLCHAIN_SHA256: the toolchain .deb is a *continuous* prerelease asset;
#   when upstream replaces it the checksum fails loudly instead of silently
#   changing compilers. Bump URL + checksum together, then re-run
#   `cargo xtask test n64` (goldens, perf ceilings).
# The former base, ghcr.io/dragonminded/libdragon:trunk, became unavailable
# (registry: denied) in 2026-10.

FROM ubuntu:24.04

ARG TOOLCHAIN_URL=https://github.com/DragonMinded/libdragon/releases/download/toolchain-continuous-prerelease/gcc-toolchain-mips64-x86_64.deb
# gcc-toolchain-mips64 16.2.0.34909605681 (asset updated 2026-09-15).
ARG TOOLCHAIN_SHA256=fac8e6572493a66468b7d45df41b042f90b1a630ec96aa218bd5fb1f320a0a4d
ARG LIBDRAGON_COMMIT=2c3d5db653f4b46f8bc97f033fb29e126cafd60c

ENV N64_INST=/opt/libdragon
ENV PATH="${N64_INST}/bin:${PATH}"

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl git make gcc g++ libc6-dev python3 file \
    && curl -fsSL -o /tmp/toolchain.deb "${TOOLCHAIN_URL}" \
    && echo "${TOOLCHAIN_SHA256}  /tmp/toolchain.deb" | sha256sum -c - \
    && apt-get install -y --no-install-recommends /tmp/toolchain.deb \
    && rm /tmp/toolchain.deb \
    && rm -rf /var/lib/apt/lists/*

# Shallow-fetch exactly the pinned commit (full clones of libdragon are
# flaky over docker's network stack).
RUN mkdir /libdragon && cd /libdragon \
    && git init -q \
    && git remote add origin https://github.com/DragonMinded/libdragon.git \
    && git fetch --depth 1 origin ${LIBDRAGON_COMMIT} \
    && git checkout -q FETCH_HEAD \
    && make -j"$(nproc)" install \
    && make -j"$(nproc)" tools-install \
    && cd / && rm -rf /libdragon

WORKDIR /workdir
