# Tracing extentd with OpenTelemetry

`fxdax-extentd` can export spans of what it does over OTLP: the requests it serves, the commits
they cause and each commit's phases, uploads to the bucket and each bucket request, store opens
and forks, extent-cache page-ins, and memory-relief passes. Tracing is off by default. While it is
off, a span costs one relaxed atomic load (measured below), and nothing is allocated or exported.
The code is `crates/extent_remote/src/trace.rs`, which makes the spans, and
`crates/fxdax/src/extentd/otel.rs`, which starts the exporter (TASK-258).

## Turning it on

| Variable | Meaning | Default |
|---|---|---|
| `FXDAX_EXTENTD_TRACES` | `otlp` turns tracing on. Any other value except empty makes the daemon refuse to start. | off |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | The collector, OTLP over HTTP with protobuf. `/v1/traces` is appended. `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` names the full URL instead. | `http://localhost:4318` |
| `OTEL_SERVICE_NAME` | The service the spans belong to. | `fxdax-extentd` |
| `OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG` | OpenTelemetry's standard sampler variables. | see Sampling |

The other `OTEL_EXPORTER_OTLP_*` variables (headers, timeout, compression) work as the
OpenTelemetry SDK documents them. Spans are exported in batches. When SIGTERM drains the daemon,
it exports what it has left before it exits.

### Sampling

Each request, each background flush, each upload and each relief pass starts a trace, and a busy
daemon starts thousands a second. So unless `OTEL_TRACES_SAMPLER` is set, the daemon samples
`parentbased_traceidratio` with ratio `0.01`: it keeps one trace in a hundred, whole. A commit's
spans follow the decision made for the request that caused it. `OTEL_TRACES_SAMPLER_ARG` alone
changes the ratio. To keep everything while debugging:

```sh
OTEL_TRACES_SAMPLER=always_on
```

## Pointing it at a collector

Jaeger's all-in-one image (Apache-2.0) takes OTLP on 4318 and serves its UI on 16686:

```sh
docker run -d --name jaeger -p 4318:4318 -p 16686:16686 jaegertracing/all-in-one:latest
FXDAX_EXTENTD_TRACES=otlp \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_TRACES_SAMPLER=always_on \
FXDAX_EXTENTD_REMOTE=... FXDAX_EXTENTD_TOKEN=... \
fxdax-extentd /run/fxdax/extentd.sock
```

Then open `http://localhost:16686` and choose the service `fxdax-extentd`. To route spans through
the OpenTelemetry Collector instead (`otel/opentelemetry-collector`, Apache-2.0), give it an `otlp`
receiver with the `http` protocol and point `OTEL_EXPORTER_OTLP_ENDPOINT` at that receiver.

## Spans

Every attribute is prefixed `fxdax.`. Branches are written in hex, as store paths name them.
Counts are integers. A failed operation has an error status carrying its message. A failed
request also has `fxdax.errno`.

| Span | Made by | Parent | Attributes |
|---|---|---|---|
| `extentd.<kind>` (`extentd.map`, `extentd.write`, `extentd.commit`, `extentd.read`, `extentd.free`, `extentd.fault`, `extentd.open`, ...) | each request a VM process sends, from its dispatch to its reply | root | `request.kind`, `branch`, `bytes` (read, write, map, unmap, private), `extents` (free, free_many, map, map_many), `durable` (commit), `store` (open, fork), `request_id` (see below), `errno` |
| `store.commit` | every commit of a writer's store | the request, or whatever commits | `branch`, `durable` |
| `commit.seal` | sealing the extents written since the last commit | `store.commit` | `extents`, `bytes` copied |
| `commit.write` | recording the commit in the open segment and queuing its flush | `store.commit` | `epoch` |
| `store.flush` | making ordered commits durable: syncs and log appends | `store.commit` if the commit is durable; root for the daemon's 50 ms flushes | `branch`, `commits` |
| `commit.upload_wait` | a commit waiting for object storage (durable: uploaded; ordered: within its lag) | `store.commit` | `epoch` |
| `store.open`, `store.create` | opening a store, or making a new one | the request or control call | `store`, `branch` |
| `store.fork` | a fork or point of a store | the request or control call | `branch`, `child`, `point` |
| `cache.page_in` | reading a part of a cached extent into memory | the fault or read that needed it | `branch`, `extent`, `arena`, `bytes` |
| `cache.relief` | an eviction pass freeing memory for an arena under pressure | root | `arena`, `bytes` evicted, `extents` evicted |
| `upload.segment` | uploading a segment, its large extents first | root, on the uploader | `branch`, `segment`, `extents`, `bytes` |
| `upload.checkpoint` | writing a branch checkpoint | root, on the uploader | `branch`, `segment`, `extents` |
| `bucket.get`, `bucket.get_range`, `bucket.head`, `bucket.list`, `bucket.list_delimited`, `bucket.create`, `bucket.update`, `bucket.overwrite`, `bucket.delete`, `bucket.delete_batch` | each object-store request (a batch of deletes: one request where the store deletes in batches, as S3's DeleteObjects) | the upload, checkpoint, page-in, open or collector pass that made it | `object_key`, `bytes`, `objects` (listings, batches of deletes), `prefixes` (listings by delimiter) |

Uploads run behind commits, so an upload is a trace of its own rather than a child of the commit
it carries. A durable commit's `commit.upload_wait` span shows how long the commit waited for
that upload.

### Request IDs

`fxdax.request_id` is set from `otel::request_id`, the one function that gives a request's ID:
the guest's number of the request the call is for (its virtio extent device's `unique`), which
the VMM's call says (wire `REQUEST`) and the daemon's log records name too (`request_id`, with
the connection's `store`; TASK-260). A call for no guest request, as one the VMM makes of its
own, has none. Work done for the request on other threads, as object storage's reads and the
pager's, reads it from `fxdax::request::current()`.

## Cost while off

`spans_off_are_nothing_and_cost_a_load` (`crates/extent_remote/src/trace.rs`) times a bucket
request's span with tracing off: started, three attributes set, ended. On Linux (aarch64, OrbStack)
it costs under 1 ns in a release build (the test's nanosecond clock reads 0 ns) and 19 ns in a
debug build, the fastest of 5 rounds of a million. With tracing off, no tracer exists and no
attribute is ever computed. The request path adds one more load per request, in
`otel::request_span`.

## Tests

- `crates/fxdax/src/extentd/tests/otel.rs` serves a write, a read and a durable commit through
  extentd into OpenTelemetry's in-memory exporter. It checks the span tree and its attributes, and
  that nothing is exported while tracing is off.
- `crates/fxdax/tests/tracing.rs` runs the `fxdax-extentd` binary against an OTLP/HTTP collector
  in the test. It checks that the spans arrive at `/v1/traces` and that nothing is sent without
  `FXDAX_EXTENTD_TRACES`.
