use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

struct DirectedLink {
    address: SocketAddr,
    blocked: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    changed: watch::Sender<u64>,
    task: JoinHandle<()>,
}

impl DirectedLink {
    async fn start(upstream: SocketAddr) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding directed fault proxy")?;
        let address = listener.local_addr()?;
        let blocked = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(0));
        let (changed, _) = watch::channel(0_u64);
        let task_blocked = blocked.clone();
        let task_changed = changed.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut downstream, _)) = listener.accept().await else {
                    return;
                };
                if task_blocked.load(Ordering::Acquire) {
                    drop(downstream);
                    continue;
                }
                let mut changes = task_changed.subscribe();
                tokio::spawn(async move {
                    let Ok(mut upstream_stream) = TcpStream::connect(upstream).await else {
                        return;
                    };
                    tokio::select! {
                        _ = copy_bidirectional(&mut downstream, &mut upstream_stream) => {}
                        _ = changes.changed() => {}
                    }
                });
            }
        });
        Ok(Self {
            address,
            blocked,
            generation,
            changed,
            task,
        })
    }

    fn set_blocked(&self, blocked: bool) {
        self.blocked.store(blocked, Ordering::Release);
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.changed.send_replace(generation);
    }
}

impl Drop for DirectedLink {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A controller-owned TCP fabric with one proxy for every directed Raft link.
/// Blocking a link also closes already-established keep-alive connections.
pub struct FaultNetwork {
    links: BTreeMap<(u64, u64), DirectedLink>,
}

impl FaultNetwork {
    pub async fn start(peer_addresses: &BTreeMap<u64, SocketAddr>) -> Result<Self> {
        let mut links = BTreeMap::new();
        for source in peer_addresses.keys().copied() {
            for (target, upstream) in peer_addresses {
                if source == *target {
                    continue;
                }
                let link = DirectedLink::start(*upstream)
                    .await
                    .with_context(|| format!("starting fault link {source}->{target}"))?;
                links.insert((source, *target), link);
            }
        }
        Ok(Self { links })
    }

    pub fn routes_for(&self, source: u64) -> BTreeMap<u64, String> {
        self.links
            .iter()
            .filter(|((link_source, _), _)| *link_source == source)
            .map(|((_, target), link)| (*target, format!("http://{}", link.address)))
            .collect()
    }

    pub fn cut(&self, source: u64, target: u64) -> Result<()> {
        self.link(source, target)?.set_blocked(true);
        Ok(())
    }

    pub fn restore(&self, source: u64, target: u64) -> Result<()> {
        self.link(source, target)?.set_blocked(false);
        Ok(())
    }

    pub fn isolate(&self, node: u64) {
        for ((source, target), link) in &self.links {
            if *source == node || *target == node {
                link.set_blocked(true);
            }
        }
    }

    pub fn partition(&self, groups: &[BTreeSet<u64>]) {
        for ((source, target), link) in &self.links {
            let same_group = groups
                .iter()
                .any(|group| group.contains(source) && group.contains(target));
            link.set_blocked(!same_group);
        }
    }

    pub fn heal(&self) {
        for link in self.links.values() {
            link.set_blocked(false);
        }
    }

    fn link(&self, source: u64, target: u64) -> Result<&DirectedLink> {
        self.links.get(&(source, target)).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no directed fault link {source}->{target}"),
            )
            .into()
        })
    }
}
