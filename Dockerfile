# Build stage: templates and static files are embedded at compile time.
FROM rust:1.99.1-slim-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock build.rs ./
COPY settings.toml ./
COPY src ./src
COPY templates ./templates
COPY static ./static
RUN cargo build --release --locked

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --no-install-recommends -y poppler-utils poppler-data \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home searxng
WORKDIR /app
COPY --chmod=755 --from=build /app/target/release/rustxng /usr/local/bin/rustxng
COPY --chmod=644 --from=build /app/settings.toml /app/settings.toml
COPY --chmod=644 LICENSE NOTICE /app/
USER searxng
ENV SEARXNG_SETTINGS_PATH=/app/settings.toml
EXPOSE 29483
ENTRYPOINT ["/usr/local/bin/rustxng"]
CMD ["--bind", "0.0.0.0", "--port", "29483"]
