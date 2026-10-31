# Headless image for sp404-server. The UI crate is not built or copied in.
#
# Rust 1.83.0 is the compiler this workspace resolves with. hyper-util stays
# at 0.1.20 and encoding_rs at 0.8.35; those pins are in crates/sp404-server
# and are locked by `cargo build --locked`.
#
# SP404_API_KEY is not set here. The process exits if it is missing at runtime.

FROM rust:1.83.0-bookworm AS build

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY examples ./examples
RUN cargo build --locked --release -p sp404-server

FROM debian:bookworm-slim

COPY --from=build /src/target/release/sp404-server /usr/local/bin/sp404-server

# The binary default is 127.0.0.1:4040. A published container port accepts
# connections only when SP404_BIND is a non-loopback address such as 0.0.0.0:4040.
EXPOSE 4040

USER nobody
ENTRYPOINT ["/usr/local/bin/sp404-server"]
