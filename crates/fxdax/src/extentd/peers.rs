// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the daemon's part in the hosts sharing its bucket (TASK-406.2, `crate::remote::peers`):
//! it serves them parts of the bucket's objects over gRPC, at `FXDAX_EXTENTD_PEER_ADDRESS`
//! (`proto/fxdax/peer/v1/peer.proto`), and its stores' fetchers ask them for the parts they own,
//! `FXDAX_EXTENTD_PEERS` naming them at its start and the control API's `SetPeers` at any time.
//! What it serves comes from the host's memory and part cache, which it needs, or from the bucket
//! once, however many hosts ask at once ([`crate::remote::serving_peers`]).

use crate::remote::peers::{Asked, Link, Peers};
use extent_frame::{Encoding, Stored};
use futures::FutureExt as _;
use futures::future::BoxFuture;
use proto::peer_client::PeerClient;
use proto::peer_server::PeerServer;
use proto::{GetPartRequest, GetPartResponse};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tonic::{Code, Request, Response, Status};

/// What hosts sharing a bucket serve each other, generated from its .proto.
pub mod proto {
    tonic::include_proto!("fxdax.peer.v1");
}

/// How long a host waits to connect to a peer: on the hosts' own network, a peer slower than this
/// is as good as down.
const CONNECT: Duration = Duration::from_secs(1);

/// How long a host waits for a part a peer serves: one the peer fetches from the bucket itself,
/// behind others in its budget, takes the bucket's latency and more.
const DEADLINE: Duration = Duration::from_secs(5);

/// How many threads serve the peers' requests, whose fetches run on the remotes' runtime.
const THREADS: usize = 2;

/// The parts the daemon served its peers, and those it had not as they asked.
static SERVED: AtomicU64 = AtomicU64::new(0);
static SERVED_MISSING: AtomicU64 = AtomicU64::new(0);

/// Has the daemon serve the hosts sharing its bucket, at `address` (`FXDAX_EXTENTD_PEER_ADDRESS`),
/// on `listener`, bound there, and its stores' fetchers ask them, `peers` at first
/// (`FXDAX_EXTENTD_PEERS`), for the parts they own: before any store opens. It needs the host's
/// part cache, which keeps what it serves.
pub fn start(
    address: &str,
    listener: std::net::TcpListener,
    peers: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<(), String> {
    let peers: Vec<String> = peers.into_iter().map(|peer| peer.as_ref().to_owned()).collect();
    for peer in std::iter::once(address).chain(peers.iter().map(String::as_str)) {
        check_address(peer)?;
    }
    let url = crate::remote::Host::from_env()?.bucket;
    let failed = |what: &str| format!("serving peers: {what}");
    let bucket = super::stores::remote_bucket().map_err(|err| failed(&format!("{err:?}")))?;
    let cache = super::stores::remote_cache().map_err(|err| failed(&format!("{err:?}")))?;
    let cache = cache.ok_or_else(|| {
        "FXDAX_EXTENTD_PEER_ADDRESS is set, but FXDAX_EXTENTD_CACHE names no directory for the \
         part cache, which keeps what the daemon serves its peers"
            .to_owned()
    })?;
    let linked = url.clone();
    let asking = Peers::new(address, move |peer| link(peer, &linked));
    asking.set(peers);
    crate::remote::ask_peers(bucket.clone(), asking)?;
    let server = Server { bucket: url, serving: crate::remote::serving_peers(bucket, cache) };
    listener.set_nonblocking(true).map_err(|err| failed(&err.to_string()))?;
    let serving = std::thread::Builder::new().name("extentd peers".into()).spawn(move || {
        if let Err(err) = serve(listener, server) {
            log::error!("extentd: serving peers: {err}");
            std::process::exit(1);
        }
    });
    serving.map(drop).map_err(|err| failed(&err.to_string()))
}

/// Has the hosts sharing the bucket be those at `addresses` (`SetPeers`): `EINVAL` if one is no
/// `HOST:PORT`, `ENOTSUP` if the daemon serves no peers.
pub(super) fn set(addresses: Vec<String>) -> Result<(), crate::Errno> {
    let peers = crate::remote::peers().ok_or(crate::Errno::ENOTSUP)?;
    for address in &addresses {
        check_address(address).map_err(|err| {
            log::warn!("extentd: SetPeers: {err}");
            crate::Errno::EINVAL
        })?;
    }
    log::info!("extentd: peers: {}", addresses.join(","));
    peers.set(addresses);
    Ok(())
}

/// Whether `address` is a host's `HOST:PORT`, as a peer is reached at.
fn check_address(address: &str) -> Result<(), String> {
    let uri: tonic::codegen::http::Uri =
        format!("http://{address}").parse().map_err(|err| format!("peer {address:?}: {err}"))?;
    match (uri.host(), uri.port_u16(), uri.path()) {
        (Some(host), Some(_), "/")
            if !host.is_empty() && uri.authority().is_some_and(|at| at.as_str() == address) =>
        {
            Ok(())
        }
        _ => Err(format!("peer {address:?}: not HOST:PORT")),
    }
}

/// What the daemon's stores' fetchers asked its peers, and the parts it served them and had not as
/// they asked, if it serves peers.
pub(super) fn counts() -> Option<(crate::remote::peers::Counts, u64, u64)> {
    let counts = crate::remote::peers()?.counts();
    Some((counts, SERVED.load(Ordering::Relaxed), SERVED_MISSING.load(Ordering::Relaxed)))
}

/// The daemon's line in its stats file (`FXDAX_EXTENTD_STATS`): its [`counts`].
pub(super) fn line() -> Option<String> {
    let (counts, served, served_missing) = counts()?;
    let crate::remote::peers::Counts { asked, given, missing, failed } = counts;
    Some(format!(
        "peers: asked={asked} given={given} missing={missing} failed={failed} served={served} \
         served-missing={served_missing}"
    ))
}

/// Serves `server` to the peers that connect to `listener`, until accepting fails.
fn serve(listener: std::net::TcpListener, server: Server) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(THREADS)
        .thread_name("extentd peers")
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        tonic::transport::Server::builder()
            .tcp_nodelay(true)
            .add_service(PeerServer::new(server))
            .serve_with_incoming(incoming)
            .await
            .map_err(std::io::Error::other)
    })
}

/// What serves the daemon's peers: the parts of `bucket`, by URL, which `serving` reads.
struct Server {
    bucket: String,
    serving: crate::remote::Serving,
}

#[tonic::async_trait]
impl proto::peer_server::Peer for Server {
    async fn get_part(
        &self,
        request: Request<GetPartRequest>,
    ) -> Result<Response<GetPartResponse>, Status> {
        let GetPartRequest { bucket, key, part, offset, len, crc, encoding } = request.into_inner();
        if bucket != self.bucket {
            let theirs = format!("asked of {bucket}, this host's bucket being {}", self.bucket);
            return Err(Status::failed_precondition(theirs));
        }
        if !extent_remote::peer::served(&key) {
            return Err(Status::invalid_argument(format!("{key}: no parts served")));
        }
        let encoding = match proto::Encoding::try_from(encoding) {
            Ok(proto::Encoding::Raw) => Encoding::Raw,
            Ok(proto::Encoding::Zstd) => Encoding::Zstd,
            Err(_) => return Err(Status::invalid_argument(format!("encoding {encoding}"))),
        };
        if len == 0 || len > 2 * extent_remote::layout::PART {
            return Err(Status::invalid_argument(format!("part {part} of {key}, of {len} bytes")));
        }
        let asked = Asked { key, part, stored: Stored { offset, len, crc, encoding } };
        match self.serving.part(asked).await {
            Ok(Some(bytes)) => {
                SERVED.fetch_add(1, Ordering::Relaxed);
                Ok(Response::new(GetPartResponse { bytes: bytes.to_vec() }))
            }
            Ok(None) => {
                SERVED_MISSING.fetch_add(1, Ordering::Relaxed);
                Err(Status::not_found(format!("part {part}: not as asked")))
            }
            Err(err) => {
                log::warn!("extentd: serving a peer part {part}: {err}");
                Err(Status::unavailable(err.to_string()))
            }
        }
    }
}

/// The link to the peer at `address`, a daemon serving the parts of the bucket at `bucket`, by URL,
/// connected when first asked, on the remotes' runtime.
fn link(address: &str, bucket: &str) -> Arc<dyn Link> {
    let runtime = crate::remote::runtime().handle;
    let _entered = runtime.enter();
    let endpoint = tonic::transport::Endpoint::from_shared(format!("http://{address}"))
        .expect("a peer's address checked")
        .connect_timeout(CONNECT)
        .timeout(DEADLINE)
        .tcp_nodelay(true);
    let client = PeerClient::new(endpoint.connect_lazy());
    Arc::new(Grpc { client, bucket: bucket.to_owned() })
}

/// A peer reached over gRPC.
struct Grpc {
    client: PeerClient<tonic::transport::Channel>,
    bucket: String,
}

impl Link for Grpc {
    fn get(&self, asked: Asked) -> BoxFuture<'static, Result<Option<Vec<u8>>, String>> {
        let Asked { key, part, stored } = asked;
        let encoding = match stored.encoding {
            Encoding::Raw => proto::Encoding::Raw,
            Encoding::Zstd => proto::Encoding::Zstd,
        };
        let request = GetPartRequest {
            bucket: self.bucket.clone(),
            key,
            part,
            offset: stored.offset,
            len: stored.len,
            crc: stored.crc,
            encoding: encoding.into(),
        };
        let mut client = self.client.clone();
        async move {
            match client.get_part(request).await {
                Ok(response) => Ok(Some(response.into_inner().bytes)),
                Err(status) if status.code() == Code::NotFound => Ok(None),
                Err(status) => Err(format!("{:?}: {}", status.code(), status.message())),
            }
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests;
