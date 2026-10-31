# Deploying sp404-server

The container runs the headless HTTP server only. It does not include `sp404-ui`. The image build compiles `sp404-server` and leaves the UI crate out of the runtime image.

The binary default for `SP404_BIND` is `127.0.0.1:4040`. Inside a container that address accepts connections from the container itself, not from the host. Set `SP404_BIND=0.0.0.0:4040` when you publish the port.

`SP404_API_KEY` is not baked into the image. Pass it at runtime. If it is missing or empty, the process prints an error and exits.

The image uses Rust 1.83.0, the compiler this workspace resolves with. `hyper-util` 0.1.20 and `encoding_rs` 0.8.35 stay pinned; `cargo build --locked` keeps that lockfile.

## Build and run

```bash
docker build -t sp404-server .
docker run --rm -p 4040:4040 \
  -e SP404_API_KEY=dev \
  -e SP404_BIND=0.0.0.0:4040 \
  sp404-server
```

Optional limits. Omit them to keep the defaults.

| Variable | Default |
| --- | --- |
| `SP404_BIND` | `127.0.0.1:4040` (set `0.0.0.0:4040` in the container) |
| `SP404_MAX_BARS` | `64` |
| `SP404_MAX_UPLOAD_BYTES` | `20971520` (20 MiB) |
| `SP404_MAX_CONCURRENT_JOBS` | `2` |

A request over the bar limit or the upload limit is `413`. When that many jobs are already queued or running, `POST /jobs` is `429`.

Jobs are stored in memory. Restarting the container drops them.

## Requests

`GET /health` does not need a key. Every other route needs `Authorization: Bearer <key>`.

```bash
curl -s localhost:4040/health

curl -s -H "Authorization: Bearer dev" \
  -F audio=@loop.wav \
  -F style=house \
  -F seed=1 \
  -F bars=8 \
  -F mode=transient \
  localhost:4040/jobs
```

`style` is `house`, `boom-bap`, or `techno`. `mode` is `transient`, `beat-grid`, or `phrase`. For `phrase`, add `-F phrase_bars=1` (or `2` or `4`). The `POST` responds `202` with a job id, for example `job-1`.

```bash
curl -s -H "Authorization: Bearer dev" localhost:4040/jobs/job-1
curl -s -H "Authorization: Bearer dev" localhost:4040/jobs/job-1/plan
curl -s -H "Authorization: Bearer dev" -o song.wav localhost:4040/jobs/job-1/audio
```

`GET /jobs/:id` reports `queued`, `running`, `succeeded`, or `failed`. The plan and audio routes return the arrangement JSON and a WAV after success, and `409` before that. An unknown id is `404`.
