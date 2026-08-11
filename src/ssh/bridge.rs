use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;
use tokio::time::{Instant, sleep_until, timeout};

const COPY_BUFFER_SIZE: usize = 16 * 1024;

#[derive(Debug, Clone, Copy)]
pub(super) struct BridgeTimeouts {
    pub write_stall: Duration,
    pub half_close_idle: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BridgeDirection {
    LocalToSsh,
    SshToLocal,
}

impl fmt::Display for BridgeDirection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalToSsh => formatter.write_str("local_to_ssh"),
            Self::SshToLocal => formatter.write_str("ssh_to_local"),
        }
    }
}

#[derive(Debug)]
pub(super) enum BridgeOutcome {
    Completed {
        stats: BridgeStats,
    },
    Cancelled {
        stats: BridgeStats,
    },
    WriteStall {
        direction: BridgeDirection,
        operation: &'static str,
        stats: BridgeStats,
    },
    HalfCloseIdle {
        waiting_for: BridgeDirection,
        stats: BridgeStats,
    },
    IoError {
        direction: BridgeDirection,
        operation: &'static str,
        error: io::Error,
        stats: BridgeStats,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct BridgeStats {
    pub local_to_ssh_bytes: u64,
    pub ssh_to_local_bytes: u64,
}

impl BridgeOutcome {
    pub fn stats(&self) -> BridgeStats {
        match self {
            Self::Completed { stats }
            | Self::Cancelled { stats }
            | Self::WriteStall { stats, .. }
            | Self::HalfCloseIdle { stats, .. }
            | Self::IoError { stats, .. } => *stats,
        }
    }
}

#[derive(Debug)]
enum DirectionFailure {
    WriteStall {
        direction: BridgeDirection,
        operation: &'static str,
    },
    IoError {
        direction: BridgeDirection,
        operation: &'static str,
        error: io::Error,
    },
}

#[derive(Debug, Clone, Copy)]
struct DirectionComplete {
    bytes: u64,
    eof_at: Instant,
}

impl DirectionFailure {
    fn into_outcome(self, stats: BridgeStats) -> BridgeOutcome {
        match self {
            Self::WriteStall {
                direction,
                operation,
            } => BridgeOutcome::WriteStall {
                direction,
                operation,
                stats,
            },
            Self::IoError {
                direction,
                operation,
                error,
            } => BridgeOutcome::IoError {
                direction,
                operation,
                error,
                stats,
            },
        }
    }
}

pub(super) async fn run<L, S>(
    local: L,
    ssh: S,
    timeouts: BridgeTimeouts,
    mut shutdown: watch::Receiver<bool>,
) -> BridgeOutcome
where
    L: AsyncRead + AsyncWrite + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (local_reader, local_writer) = tokio::io::split(local);
    let (ssh_reader, ssh_writer) = tokio::io::split(ssh);
    let (local_to_ssh_progress_tx, mut local_to_ssh_progress_rx) = watch::channel(Instant::now());
    let (ssh_to_local_progress_tx, mut ssh_to_local_progress_rx) = watch::channel(Instant::now());
    let local_to_ssh_counter = AtomicU64::new(0);
    let ssh_to_local_counter = AtomicU64::new(0);

    let local_to_ssh = copy_direction(
        local_reader,
        ssh_writer,
        BridgeDirection::LocalToSsh,
        timeouts.write_stall,
        local_to_ssh_progress_tx,
        &local_to_ssh_counter,
    );
    let ssh_to_local = copy_direction(
        ssh_reader,
        local_writer,
        BridgeDirection::SshToLocal,
        timeouts.write_stall,
        ssh_to_local_progress_tx,
        &ssh_to_local_counter,
    );
    tokio::pin!(local_to_ssh);
    tokio::pin!(ssh_to_local);

    tokio::select! {
        biased;
        () = wait_for_shutdown(&mut shutdown) => BridgeOutcome::Cancelled {
            stats: load_stats(&local_to_ssh_counter, &ssh_to_local_counter),
        },
        first = &mut local_to_ssh => {
            let local_to_ssh = match first {
                Ok(completed) => completed,
                Err(failure) => {
                    return failure.into_outcome(load_stats(
                        &local_to_ssh_counter,
                        &ssh_to_local_counter,
                    ));
                }
            };
            match await_after_half_close(
                ssh_to_local.as_mut(),
                &mut ssh_to_local_progress_rx,
                BridgeDirection::SshToLocal,
                local_to_ssh.eof_at,
                timeouts.half_close_idle,
                &mut shutdown,
            ).await {
                Ok(ssh_to_local) => BridgeOutcome::Completed {
                    stats: BridgeStats {
                        local_to_ssh_bytes: local_to_ssh.bytes,
                        ssh_to_local_bytes: ssh_to_local.bytes,
                    },
                },
                Err(failure) => half_close_failure_outcome(
                    failure,
                    &local_to_ssh_counter,
                    &ssh_to_local_counter,
                ),
            }
        }
        first = &mut ssh_to_local => {
            let ssh_to_local = match first {
                Ok(completed) => completed,
                Err(failure) => {
                    return failure.into_outcome(load_stats(
                        &local_to_ssh_counter,
                        &ssh_to_local_counter,
                    ));
                }
            };
            match await_after_half_close(
                local_to_ssh.as_mut(),
                &mut local_to_ssh_progress_rx,
                BridgeDirection::LocalToSsh,
                ssh_to_local.eof_at,
                timeouts.half_close_idle,
                &mut shutdown,
            ).await {
                Ok(local_to_ssh) => BridgeOutcome::Completed {
                    stats: BridgeStats {
                        local_to_ssh_bytes: local_to_ssh.bytes,
                        ssh_to_local_bytes: ssh_to_local.bytes,
                    },
                },
                Err(failure) => half_close_failure_outcome(
                    failure,
                    &local_to_ssh_counter,
                    &ssh_to_local_counter,
                ),
            }
        }
    }
}

fn load_stats(local_to_ssh: &AtomicU64, ssh_to_local: &AtomicU64) -> BridgeStats {
    BridgeStats {
        local_to_ssh_bytes: local_to_ssh.load(Ordering::Relaxed),
        ssh_to_local_bytes: ssh_to_local.load(Ordering::Relaxed),
    }
}

enum HalfCloseFailure {
    Cancelled,
    Idle { waiting_for: BridgeDirection },
    Direction(DirectionFailure),
}

fn half_close_failure_outcome(
    failure: HalfCloseFailure,
    local_to_ssh: &AtomicU64,
    ssh_to_local: &AtomicU64,
) -> BridgeOutcome {
    let stats = load_stats(local_to_ssh, ssh_to_local);
    match failure {
        HalfCloseFailure::Cancelled => BridgeOutcome::Cancelled { stats },
        HalfCloseFailure::Idle { waiting_for } => {
            BridgeOutcome::HalfCloseIdle { waiting_for, stats }
        }
        HalfCloseFailure::Direction(failure) => failure.into_outcome(stats),
    }
}

async fn copy_direction<R, W>(
    mut reader: R,
    mut writer: W,
    direction: BridgeDirection,
    write_stall: Duration,
    progress: watch::Sender<Instant>,
    copied_bytes: &AtomicU64,
) -> Result<DirectionComplete, DirectionFailure>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    let mut copied = 0_u64;

    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| DirectionFailure::IoError {
                direction,
                operation: "read",
                error,
            })?;
        if read == 0 {
            let eof_at = Instant::now();
            return match timeout(write_stall, writer.shutdown()).await {
                Ok(Ok(())) => Ok(DirectionComplete {
                    bytes: copied,
                    eof_at,
                }),
                Ok(Err(error)) => Err(DirectionFailure::IoError {
                    direction,
                    operation: "shutdown",
                    error,
                }),
                Err(_) => Err(DirectionFailure::WriteStall {
                    direction,
                    operation: "shutdown",
                }),
            };
        }
        progress.send_replace(Instant::now());

        let mut offset = 0;
        while offset < read {
            let written = match timeout(write_stall, writer.write(&buffer[offset..read])).await {
                Ok(Ok(0)) => {
                    return Err(DirectionFailure::IoError {
                        direction,
                        operation: "write",
                        error: io::Error::new(
                            io::ErrorKind::WriteZero,
                            "bridge writer returned zero",
                        ),
                    });
                }
                Ok(Ok(written)) => written,
                Ok(Err(error)) => {
                    return Err(DirectionFailure::IoError {
                        direction,
                        operation: "write",
                        error,
                    });
                }
                Err(_) => {
                    return Err(DirectionFailure::WriteStall {
                        direction,
                        operation: "write",
                    });
                }
            };
            offset += written;
            copied = copied.saturating_add(written as u64);
            copied_bytes.store(copied, Ordering::Relaxed);
            progress.send_replace(Instant::now());
        }
    }
}

async fn await_after_half_close<F>(
    mut remaining: Pin<&mut F>,
    progress: &mut watch::Receiver<Instant>,
    direction: BridgeDirection,
    half_close_started: Instant,
    idle_timeout: Duration,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<DirectionComplete, HalfCloseFailure>
where
    F: Future<Output = Result<DirectionComplete, DirectionFailure>>,
{
    loop {
        let last_progress = *progress.borrow_and_update();
        let deadline = half_close_started.max(last_progress) + idle_timeout;
        tokio::select! {
            biased;
            () = wait_for_shutdown(shutdown) => return Err(HalfCloseFailure::Cancelled),
            result = &mut remaining => return result.map_err(HalfCloseFailure::Direction),
            changed = progress.changed() => {
                if changed.is_err() {
                    return remaining.await.map_err(HalfCloseFailure::Direction);
                }
            }
            () = sleep_until(deadline) => {
                return Err(HalfCloseFailure::Idle {
                    waiting_for: direction,
                });
            }
        }
    }
}

async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::task::{Context, Poll};

    use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::time::{Duration, sleep, timeout};

    use super::{
        BridgeDirection, BridgeOutcome, BridgeTimeouts, DirectionComplete, DirectionFailure,
        HalfCloseFailure, await_after_half_close, copy_direction, run,
    };

    fn timeouts(duration: Duration) -> BridgeTimeouts {
        BridgeTimeouts {
            write_stall: duration,
            half_close_idle: duration,
        }
    }

    #[tokio::test]
    async fn bridge_copies_both_directions_and_preserves_half_close() {
        let (local_bridge, mut local_peer) = tokio::io::duplex(1_024);
        let (ssh_bridge, mut ssh_peer) = tokio::io::duplex(1_024);
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run(
            local_bridge,
            ssh_bridge,
            timeouts(Duration::from_secs(1)),
            shutdown_rx,
        ));

        local_peer.write_all(b"request").await.unwrap();
        ssh_peer.write_all(b"response").await.unwrap();
        local_peer.shutdown().await.unwrap();
        ssh_peer.shutdown().await.unwrap();

        let mut at_local = Vec::new();
        let mut at_ssh = Vec::new();
        local_peer.read_to_end(&mut at_local).await.unwrap();
        ssh_peer.read_to_end(&mut at_ssh).await.unwrap();
        assert_eq!(at_local, b"response");
        assert_eq!(at_ssh, b"request");

        let outcome = task.await.unwrap();
        assert!(matches!(
            outcome,
            BridgeOutcome::Completed { stats }
                if stats.local_to_ssh_bytes == 7 && stats.ssh_to_local_bytes == 8
        ));
    }

    #[tokio::test]
    async fn fully_idle_bridge_is_not_a_write_stall() {
        let (local_bridge, _local_peer) = tokio::io::duplex(64);
        let (ssh_bridge, _ssh_peer) = tokio::io::duplex(64);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run(
            local_bridge,
            ssh_bridge,
            timeouts(Duration::from_millis(30)),
            shutdown_rx,
        ));

        sleep(Duration::from_millis(100)).await;
        assert!(!task.is_finished(), "an idle bridge must remain open");
        shutdown_tx.send(true).unwrap();
        assert!(matches!(
            timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap(),
            BridgeOutcome::Cancelled { .. }
        ));
    }

    #[tokio::test]
    async fn route_shutdown_cancels_an_active_bridge() {
        let (local_bridge, _local_peer) = tokio::io::duplex(64);
        let (ssh_bridge, _ssh_peer) = tokio::io::duplex(64);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run(
            local_bridge,
            ssh_bridge,
            timeouts(Duration::from_secs(1)),
            shutdown_rx,
        ));

        shutdown_tx.send(true).unwrap();
        assert!(matches!(
            timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap(),
            BridgeOutcome::Cancelled { .. }
        ));
    }

    #[tokio::test]
    async fn half_close_idle_timer_starts_at_eof() {
        let (local_bridge, mut local_peer) = tokio::io::duplex(64);
        let (ssh_bridge, _ssh_peer) = tokio::io::duplex(64);
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run(
            local_bridge,
            ssh_bridge,
            timeouts(Duration::from_millis(40)),
            shutdown_rx,
        ));

        sleep(Duration::from_millis(80)).await;
        assert!(
            !task.is_finished(),
            "full-duplex idle must not start the timer"
        );
        local_peer.shutdown().await.unwrap();
        sleep(Duration::from_millis(15)).await;
        assert!(
            !task.is_finished(),
            "the stale pre-EOF timestamp was reused"
        );
        assert!(matches!(
            timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap(),
            BridgeOutcome::HalfCloseIdle {
                waiting_for: BridgeDirection::SshToLocal,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn half_close_deadline_includes_first_direction_shutdown_time() {
        let eof_at = tokio::time::Instant::now() - Duration::from_millis(100);
        let (_progress_tx, mut progress_rx) = tokio::sync::watch::channel(eof_at);
        let (_shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let remaining = std::future::pending::<Result<DirectionComplete, DirectionFailure>>();
        tokio::pin!(remaining);

        let failure = timeout(
            Duration::from_millis(50),
            await_after_half_close(
                remaining.as_mut(),
                &mut progress_rx,
                BridgeDirection::SshToLocal,
                eof_at,
                Duration::from_millis(20),
                &mut shutdown_rx,
            ),
        )
        .await
        .expect("elapsed half-close deadline must fire immediately")
        .expect_err("pending remaining direction must time out");
        assert!(matches!(
            failure,
            HalfCloseFailure::Idle {
                waiting_for: BridgeDirection::SshToLocal
            }
        ));
    }

    #[tokio::test]
    async fn half_close_idle_deadline_is_extended_by_progress() {
        let (local_bridge, mut local_peer) = tokio::io::duplex(64);
        let (ssh_bridge, mut ssh_peer) = tokio::io::duplex(64);
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run(
            local_bridge,
            ssh_bridge,
            timeouts(Duration::from_millis(50)),
            shutdown_rx,
        ));

        local_peer.shutdown().await.unwrap();
        for byte in [b'a', b'b', b'c'] {
            sleep(Duration::from_millis(30)).await;
            ssh_peer.write_all(&[byte]).await.unwrap();
        }
        ssh_peer.shutdown().await.unwrap();

        let mut received = Vec::new();
        local_peer.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, b"abc");
        assert!(matches!(
            timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap(),
            BridgeOutcome::Completed { stats }
                if stats.local_to_ssh_bytes == 0 && stats.ssh_to_local_bytes == 3
        ));
    }

    #[tokio::test]
    async fn pending_write_times_out_without_busy_polling() {
        let polls = Arc::new(AtomicUsize::new(0));
        let writer = NeverWritable {
            polls: polls.clone(),
        };
        let (progress_tx, _progress_rx) = tokio::sync::watch::channel(tokio::time::Instant::now());
        let copied = AtomicU64::new(0);

        let outcome = copy_direction(
            tokio::io::repeat(1).take(1),
            writer,
            BridgeDirection::LocalToSsh,
            Duration::from_millis(30),
            progress_tx,
            &copied,
        )
        .await;
        assert!(matches!(
            outcome,
            Err(DirectionFailure::WriteStall {
                direction: BridgeDirection::LocalToSsh,
                operation: "write"
            })
        ));
        assert!(polls.load(Ordering::Relaxed) <= 3);
        assert_eq!(copied.load(Ordering::Relaxed), 0);
    }

    struct NeverWritable {
        polls: Arc<AtomicUsize>,
    }

    impl AsyncWrite for NeverWritable {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.polls.fetch_add(1, Ordering::Relaxed);
            Poll::Pending
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}
