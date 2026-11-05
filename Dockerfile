# Kern in a container (docs/research/RUST_PORT_PLAN.md, R6). Three stages:
#   build   the Rust toolchain, with Z3 5.3.3 compiled from source and linked statically (`static-z3`):
#           distribution libz3 packages aren't 5.3.3, and the proofs must run on the version the oracle used
#   kern    the image to use: just the binary
#   check   the acceptance test: the Python oracle and the diff harness, run against the binary inside the
#           container (`docker build --target check .` fails unless every module agrees)
#
#   docker build -t kern .
#   docker run --rm -v "$PWD:/work" kern check examples/shop --quick
#   docker run --rm -i -v "$PWD:/work" kern mcp                       (the MCP server on stdio)
#   docker run --rm -p 8080:8080 -v "$PWD:/work" kern host serve examples/shop --bind 0.0.0.0
#
# Token counts in `view` need the Claude tokenizer: mount it and set KERN_TOKENIZER=/path/tokenizer.json.
# `kern build` (compiling to a native binary) needs cargo, which the slim image doesn't have.

# trixie: Z3 5.3.3 needs C++20 <format> (GCC >= 13) and CMake >= 3.26; bookworm has GCC 12 and CMake 3.25.
# All three images are trixie, so the binary's glibc matches where it runs.
FROM rust:1.99-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY toolchain toolchain
COPY runtime runtime
COPY validation validation
RUN cargo build --release --locked -p kern --features static-z3

FROM debian:trixie-slim AS kern
COPY --from=build /src/target/release/kern /usr/local/bin/kern
WORKDIR /work
ENTRYPOINT ["kern"]

FROM python:3.9-slim-trixie AS check
RUN pip install --no-cache-dir z3-solver==5.3.3.0
COPY --from=build /src/target/release/kern /usr/local/bin/kern
WORKDIR /repo
COPY kern kern
COPY validation validation
COPY examples examples
COPY tests/probes tests/probes
COPY docs docs
RUN python validation/oracle_analyse.py diff --rust /usr/local/bin/kern
