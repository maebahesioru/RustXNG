# Build stage: templates and static files are embedded at compile time.
FROM rust:1.99.1-slim-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock build.rs ./
COPY settings.toml settings.full.toml settings.clean.toml settings.api.toml ./
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
# The defaults and the profiles (--profile) are built into the binary. This
# file only holds overrides; mount your own over it.
RUN printf '%s\n' '# Overrides laid over the built-in defaults and the chosen --profile.' \
    '# Mount your own file here; put only the values you change.' > /app/settings.toml \
 && chmod 644 /app/settings.toml
COPY --chmod=644 LICENSE NOTICE /app/
USER searxng
ENV SEARXNG_SETTINGS_PATH=/app/settings.toml
EXPOSE 29483
ENTRYPOINT ["/usr/local/bin/rustxng"]
CMD ["--bind", "0.0.0.0", "--port", "29483"]
