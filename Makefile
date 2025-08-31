.PHONY: run build docker

run:
	UPSTREAM_BASE=https://cloud.r-project.org LISTEN_ADDR=0.0.0.0:8080 RUST_LOG=info \
		cargo run --release

build:
	cargo build --release

docker:
	docker build -t smartcran-logger:latest .
