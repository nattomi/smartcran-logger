# Smart CRAN Logger (Rust)

A tiny **transparent proxy** that logs what R clients (`install.packages()`, `renv`, `pak`, etc.) request from CRAN and forwards those requests to a real mirror.

- No policy, no rewriting, no caching — just structured JSON logs.

---

## What it does

- Accepts CRAN-style paths (e.g. `/src/contrib/PACKAGES.rds`, `/src/contrib/<pkg>_<ver>.tar.gz`).
- Forwards each request to an upstream CRAN mirror and **streams** the response back.
- Emits **one JSON log line per request** (path, status, latency, ETag, UA, etc.).
- Parses CRAN paths into `artifact_type`, `package`, `version`, and (for binaries) `r_minor`, `os`.

> Source tarballs do not encode OS or R minor version in the path; for those you will see `"r_minor": null, "os": null` in the derived fields. That’s expected.

---

## Requirements

- Docker (recommended), or a Rust toolchain (Rust ≥ 1.82) if running natively.
- An upstream CRAN mirror (default: `https://cloud.r-project.org`).

---

## Quick start (Docker, single container)

```bash
# Build
docker build -t smartcran-logger:latest .

# Run
docker run -d --name smartcran-logger -p 8080:8080 \
  -e UPSTREAM_BASE=https://cloud.r-project.org \
  -e LISTEN_ADDR=0.0.0.0:8080 \
  -e RUST_LOG=info \
  smartcran-logger:latest

# Health check
curl -f http://localhost:8080/healthz && echo OK

# Try a few endpoints
curl -I http://localhost:8080/src/contrib/PACKAGES.rds
curl -I http://localhost:8080/src/contrib/PACKAGES
curl -I http://localhost:8080/src/contrib/digest_0.6.37.tar.gz
````

Tail logs:

```bash
docker logs -f smartcran-logger
```

---

## End-to-end demo with a Docker network (R client in a separate container)

Run the logger and an R client on the same custom network:

```bash
# 1) Create network
docker network create smartcran

# 2) Build & run the logger on that network
docker build -t smartcran-logger:latest .
docker run -d \
  --name smartcran-logger \
  --network smartcran \
  -p 8080:8080 \
  -e UPSTREAM_BASE=https://cloud.r-project.org \
  -e LISTEN_ADDR=0.0.0.0:8080 \
  -e RUST_LOG=info \
  smartcran-logger:latest

# 3) Sanity checks from the host
curl -I http://localhost:8080/src/contrib/PACKAGES.rds
curl -I http://localhost:8080/src/contrib/PACKAGES
curl -I http://localhost:8080/src/contrib/assertthat_0.2.1.tar.gz

# 4) Start a minimal R container on the same network (interactive)
docker run -it --rm --network smartcran rocker/r-ver:4.4.1 R -q
```

Inside that R session:

```r
options(repos = c(CRAN = "http://smartcran-logger:8080"))
ap <- available.packages()        # hits PACKAGES.rds via the proxy
install.packages("assertthat")    # R-only package; no system toolchain required
```

Non-interactive one-liner:

```bash
docker run --rm --network smartcran rocker/r-ver:4.4.1 \
  R -q -e 'options(repos=c(CRAN="http://smartcran-logger:8080")); install.packages("assertthat")'
```

Cleanup (optional):

```bash
docker stop smartcran-logger && docker rm smartcran-logger
docker network rm smartcran
```

---

## Configuration

Environment variables:

* `UPSTREAM_BASE` — upstream CRAN mirror (default: `https://cloud.r-project.org`)
* `LISTEN_ADDR` — address for the HTTP server (default: `0.0.0.0:8080`)
* `RUST_LOG` — logging level (e.g. `info`, `debug`, `trace`)

Health:

* `GET /healthz` → `200 OK`

---

## Log format

Each request produces one JSON line to stdout (container logs). Example:

```json
{
  "timestamp": "2025-08-31T14:33:08.928842Z",
  "level": "INFO",
  "fields": {
    "message": "proxied",
    "path": "/src/contrib/assertthat_0.2.1.tar.gz",
    "status": "200",
    "latency_ms": "258",
    "ua": "R/4.4.1 R (4.4.1 x86_64-pc-linux-gnu x86_64 linux-gnu)",
    "range": "-",
    "etag_out": "\"31c6-5849be805b3c0\"",
    "content_length": "12742",
    "derived": "{\"artifact_type\":\"src_tar\",\"package\":\"assertthat\",\"version\":\"0.2.1\",\"r_minor\":null,\"os\":null}"
  }
}
```

`derived` is a JSON string with CRAN-specific fields:

* `artifact_type`: `index_rds`, `index_gz`, `index_text`, `src_tar`, `archive_tar`, `win_zip`, `mac_tgz`, or `unknown`
* `package`, `version`
* `r_minor`, `os` (only for binary paths; `null` for source tarballs)

---

## Troubleshooting

* **404 with `artifact_type:"unknown"`** — likely a path typo. Valid examples:

  * `/src/contrib/PACKAGES.rds`
  * `/src/contrib/<pkg>_<version>.tar.gz`
  * `/src/contrib/Archive/<pkg>/<pkg>_<version>.tar.gz`
* **502 / “upstream error”** — upstream mirror unreachable. Check `UPSTREAM_BASE`, DNS, firewall.
* **Slow responses** — the text `PACKAGES` is \~5–6 MB. Prefer `PACKAGES.rds`.
* **Binary packages** — Windows/macOS binary paths populate `r_minor` and `os`.

---

## Development

Run locally (Rust ≥ 1.82):

```bash
RUST_LOG=info UPSTREAM_BASE=https://cloud.r-project.org cargo run --release
```

Build the Docker image:

```bash
docker build -t smartcran-logger:latest .
```

---

## Roadmap (later)

* Prometheus `/metrics` (requests, latency, status codes)
* `/readyz` that checks the upstream mirror
* Per-tenant prefixes and tagging in logs
* Policy engine and a cache layer
