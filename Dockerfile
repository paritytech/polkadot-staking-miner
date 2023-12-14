FROM docker.io/library/ubuntu:22.04

# metadata
ARG VCS_REF
ARG BUILD_DATE

LABEL io.parity.image.authors="devops-team@parity.io" \
    io.parity.image.vendor="Parity Technologies" \
    io.parity.image.title="staking-miner-v2" \
    io.parity.image.description="Staking Miner v2 for substrate based chains" \
    io.parity.image.source="https://github.com/paritytech/staking-miner-v2/blob/${VCS_REF}/Dockerfile" \
    io.parity.image.revision="${VCS_REF}" \
    io.parity.image.created="${BUILD_DATE}" \
    io.parity.image.documentation="https://github.com/paritytech/polkadot/"

# show backtraces
ENV RUST_BACKTRACE 1

# install tools and dependencies
RUN apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install -y \
    libssl1.1 \
    ca-certificates && \
    # apt cleanup
    apt-get autoremove -y && \
    apt-get clean && \
    find /var/lib/apt/lists/ -type f -not -name lock -delete; \
    useradd -u 10000 -U -s /bin/sh miner

# add binary to docker image
COPY ./staking-miner /usr/local/bin

USER miner

ENV SEED=""
ENV URI="wss://rpc.polkadot.io"
ENV RUST_LOG="info"

# check if the binary works in this container
RUN /usr/local/bin/staking-miner --version

ENTRYPOINT [ "/usr/local/bin/staking-miner" ]
