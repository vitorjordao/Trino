# Trino's N64 build image: the official libdragon toolchain container plus
# libdragon itself (library + headers + asset/ROM tools) at a pinned commit.
#
# Rebuild (xtask does this automatically when missing):
#   docker build -t trino-n64 docker/n64
#
# Bumping libdragon = changing LIBDRAGON_COMMIT below. Never use a moving
# tag: the commit is part of the reproducible-build story.

FROM ghcr.io/dragonminded/libdragon:trunk

ARG LIBDRAGON_COMMIT=2c3d5db653f4b46f8bc97f033fb29e126cafd60c

RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates \
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
