FROM rust:1.100-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
RUN cargo build --release --locked

FROM debian:bookworm-slim
# The binary reads the system CA store for TLS and the system zoneinfo for WHOSOUT_TIME_ZONE.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-log-init --uid 10001 --no-create-home whosout \
    && install -d -o whosout /data
COPY --from=build /src/target/release/whosout /usr/local/bin/whosout
USER 10001
ENV WHOSOUT_DIR=/data \
    WHOSOUT_LISTEN=0.0.0.0:8377
VOLUME /data
EXPOSE 8377
ENTRYPOINT ["whosout"]
CMD ["serve"]
