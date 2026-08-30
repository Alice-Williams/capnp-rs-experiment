FROM rust:1-bookworm

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:${PATH}

ARG BAZELISK_VERSION=1.29.0

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        capnproto \
        curl \
        file \
        git \
        less \
        pkg-config \
        python3 \
        vim-tiny \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    curl --fail --silent --show-error --location \
        --output /tmp/bazelisk-linux-amd64 \
        "https://github.com/bazelbuild/bazelisk/releases/download/v${BAZELISK_VERSION}/bazelisk-linux-amd64"; \
    curl --fail --silent --show-error --location \
        --output /tmp/bazelisk-linux-amd64.sha256 \
        "https://github.com/bazelbuild/bazelisk/releases/download/v${BAZELISK_VERSION}/bazelisk-linux-amd64.sha256"; \
    expected_checksum=$(tr -d '[:space:]' < /tmp/bazelisk-linux-amd64.sha256); \
    actual_checksum=$(sha256sum /tmp/bazelisk-linux-amd64 | cut -d ' ' -f 1); \
    test "$expected_checksum" = "$actual_checksum"; \
    install -m 0755 /tmp/bazelisk-linux-amd64 /usr/local/bin/bazelisk; \
    ln -s /usr/local/bin/bazelisk /usr/local/bin/bazel; \
    rm -f /tmp/bazelisk-linux-amd64 /tmp/bazelisk-linux-amd64.sha256

# Debian's login shell profile replaces PATH, so make the Rust toolchain
# available in interactive shells as well as direct container commands.
RUN printf '%s\n' 'export PATH=/usr/local/cargo/bin:$PATH' > /etc/profile.d/cargo.sh

WORKDIR /workspace
CMD ["sleep", "infinity"]
