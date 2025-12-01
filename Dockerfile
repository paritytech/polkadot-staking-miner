# Runtime image expecting a pre-built binary alongside the Docker build context.
FROM ubuntu:20.04

# Metadata
ARG VCS_REF=unknown
ARG BUILD_DATE=unknown

LABEL io.parity.image.authors="devops-team@parity.io" \
      io.parity.image.vendor="Parity Technologies" \
      io.parity.image.title="polkadot-staking-miner" \
      io.parity.image.description="Polkadot staking miner for substrate based chains" \
      io.parity.image.source="https://github.com/paritytech/polkadot-staking-miner/blob/${VCS_REF}/Dockerfile" \
      io.parity.image.revision="${VCS_REF}" \
      io.parity.image.created="${BUILD_DATE}" \
      io.parity.image.documentation="https://github.com/paritytech/polkadot/"

# Backtraces & useful defaults
ENV RUST_BACKTRACE=1 \
    SEED="" \
    URI="wss://rpc.polkadot.io" \
    RUST_LOG="info"

# Install runtime dependencies (libssl1.1 still available on focal)
RUN apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install -y \
      libssl1.1 \
      ca-certificates && \
    apt-get autoremove -y && \
    apt-get clean && \
    find /var/lib/apt/lists/ -type f -not -name lock -delete && \
    useradd -u 10000 -U -s /bin/sh miner

# Copy the pre-built binary into the image
COPY ./polkadot-staking-miner /usr/local/bin

# Create writable workspace and output directories
RUN mkdir -p /workspace/results /workspace/outputs && \
    chmod 777 /workspace /workspace/results /workspace/outputs && \
    /usr/local/bin/polkadot-staking-miner --version

WORKDIR /workspace

ENTRYPOINT ["/usr/local/bin/polkadot-staking-miner"]
CMD ["--help"]
