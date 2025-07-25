mod scopeguard;

use crate::scopeguard::scopeguard::guard;
use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use log::{debug, error, info, warn};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::timeout;

static ACTIVE_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
static BYTES_TRANSFERRED: AtomicU64 = AtomicU64::new(0);

#[derive(Parser)]
#[command(name = "proxy_server")]
#[command(about = "High-performance async TCP proxy server")]
#[command(version = "1.0")]
struct Cli {
    #[arg(value_name = "TARGET_IP")]
    target_ip: IpAddr,

    #[arg(short = 'l', long = "listen-ip", default_value = "0.0.0.0")]
    listen_ip: IpAddr,

    #[arg(short = 'p', long = "listen_port", default_value = "9999")]
    listen_port: u16,

    #[arg(short = 't', long = "target_port", default_value = "80")]
    target_port: u16,

    #[arg(short = 'c', long = "max-connections", default_value = "1000")]
    max_connections: usize,

    #[arg(long = "timeout", default_value = "30")]
    timeout: u64,

    #[arg(long = "log-level", default_value = "info")]
    log_level: LogLevel,

    #[arg(long = "metrics-interval", default_value = "30")]
    metrics_interval: u64,

    #[arg(long = "buffer-size", default_value = "8192")]
    buffer_size: usize,
}

#[derive(Debug, Clone, ValueEnum)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<LogLevel> for log::LevelFilter {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Trace => log::LevelFilter::Trace,
            LogLevel::Debug => log::LevelFilter::Debug,
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Warn => log::LevelFilter::Warn,
            LogLevel::Error => log::LevelFilter::Error,
        }
    }
}

#[derive(Clone)]
struct ProxyConfig {
    target_addr: SocketAddr,
    listen_addr: SocketAddr,
    max_connection: usize,
    timeout: Duration,
    buffer_size: usize,
    metrics_interval: Duration,
}

impl ProxyConfig {
    fn from_cli(cli: Cli) -> Self {
        Self {
            target_addr: SocketAddr::new(cli.target_ip, cli.target_port),
            listen_addr: SocketAddr::new(cli.listen_ip, cli.listen_port),
            max_connection: cli.max_connections,
            timeout: Duration::from_secs(cli.timeout),
            buffer_size: cli.buffer_size,
            metrics_interval: Duration::from_secs(cli.metrics_interval),
        }
    }
}

struct ConnectionMetrics {
    id: u64,
    start_time: Instant,
    client_addr: SocketAddr,
    bytes_client_to_server: AtomicU64,
    bytes_server_to_client: AtomicU64,
}

impl ConnectionMetrics {
    fn new(id: u64, client_addr: SocketAddr) -> Self {
        Self {
            id,
            start_time: Instant::now(),
            client_addr,
            bytes_client_to_server: AtomicU64::new(0),
            bytes_server_to_client: AtomicU64::new(0),
        }
    }

    fn add_bytes_c2s(&self, bytes: u64) {
        self.bytes_client_to_server
            .fetch_add(bytes, Ordering::Relaxed);
        BYTES_TRANSFERRED.fetch_add(bytes, Ordering::Relaxed);
    }

    fn add_bytes_s2c(&self, bytes: u64) {
        self.bytes_server_to_client
            .fetch_add(bytes, Ordering::Relaxed);
        BYTES_TRANSFERRED.fetch_add(bytes, Ordering::Relaxed);
    }

    fn log_summary(&self) {
        let duration = self.start_time.elapsed();
        let c2s_bytes = self.bytes_client_to_server.load(Ordering::Relaxed);
        let s2c_bytes = self.bytes_server_to_client.load(Ordering::Relaxed);
        let total_bytes = c2s_bytes + s2c_bytes;

        info!(
            "Connections {} closed: client={}, duration={:.2}s, c2s={}B, s2c={}B, total={}B",
            self.id,
            self.client_addr,
            duration.as_secs_f64(),
            c2s_bytes,
            s2c_bytes,
            total_bytes
        );
    }
}

async fn handle_connection(
    client_stream: TcpStream,
    config: ProxyConfig,
    connection_id: u64,
    _permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    let client_addr = client_stream.peer_addr()?;
    let metrics = Arc::new(ConnectionMetrics::new(connection_id, client_addr));

    info!("Connection {} accepted from {}", connection_id, client_addr);

    let server_stream = timeout(config.timeout, TcpStream::connect(config.target_addr))
        .await
        .context("Timeout connecting to target server")?
        .context("Failed to connect to target server")?;

    debug!(
        "Connection {} established to target {}",
        connection_id, config.target_addr
    );

    let result = forward_data(
        client_stream,
        server_stream,
        metrics.clone(),
        config.buffer_size,
    )
    .await;

    metrics.log_summary();

    if let Err(e) = result {
        warn!("Connection {} error: {}", connection_id, e);
    }

    Ok(())
}

async fn forward_data(
    mut client_stream: TcpStream,
    mut server_stream: TcpStream,
    metrics: Arc<ConnectionMetrics>,
    buffer_size: usize,
) -> Result<()> {
    let (mut client_read, mut client_write) = client_stream.split();
    let (mut server_read, mut server_write) = server_stream.split();

    let metrics_c2s = metrics.clone();
    let metrics_s2c = metrics.clone();

    let client_to_server = async move {
        let mut buffer = vec![0u8; buffer_size];
        loop {
            match client_read.read(&mut buffer).await {
                Ok(0) => {
                    debug!("Connection {} client closed connection", metrics_c2s.id);
                    break;
                }
                Ok(bytes_read) => {
                    if let Err(e) = server_write.write_all(&buffer[..bytes_read]).await {
                        error!("Connection {} C2S write error: {}", metrics_c2s.id, e);
                        break;
                    }
                    metrics_c2s.add_bytes_c2s(bytes_read as u64);
                    debug!("Connection {} C2S: {} bytes", metrics_c2s.id, bytes_read);
                }
                Err(e) => {
                    error!("Connection {} C2S read error: {}", metrics_c2s.id, e);
                    break;
                }
            }
        }
        if let Err(e) = server_write.shutdown().await {
            debug!("Connection {} C2S shutdown error: {}", metrics_c2s.id, e);
        }
    };

    // server to client forwarding
    let server_to_client = async move {
        let mut buffer = vec![0u8; buffer_size];
        loop {
            match server_read.read(&mut buffer).await {
                Ok(0) => {
                    debug!("Connection {} server closed connection", metrics_s2c.id);
                    break;
                }
                Ok(bytes_read) => {
                    if let Err(e) = client_write.write_all(&buffer[..bytes_read]).await {
                        error!("Connection {} S2C write error: {}", metrics_s2c.id, e);
                        break;
                    }
                    metrics_s2c.add_bytes_s2c(bytes_read as u64);
                    debug!("Connection {} S2C: {} bytes", metrics_s2c.id, bytes_read);
                }
                Err(e) => {
                    error!("Connection {} S2C read error: {}", metrics_s2c.id, e);
                    break;
                }
            }
        }
        if let Err(e) = client_write.shutdown().await {
            debug!("Connection {} S2C shutdown error: {}", metrics_s2c.id, e);
        }
    };

    // Run both directional concurrently

    tokio::select! {
        _ = client_to_server => {},
        _ = server_to_client => {},
    }

    Ok(())
}

async fn log_metrics(interval: Duration) {
    let mut interval = tokio::time::interval(interval);
    interval.tick().await;

    loop {
        interval.tick().await;

        let active = ACTIVE_CONNECTIONS.load(Ordering::Relaxed);
        let total = TOTAL_CONNECTIONS.load(Ordering::Relaxed);
        let bytes = BYTES_TRANSFERRED.load(Ordering::Relaxed);

        info!(
            "Metrics: active_connections={}, total_connections={}, bytes_transferred={}",
            active, total, bytes
        );
    }
}

async fn run_proxy(config: ProxyConfig) -> Result<()> {
    let listener = TcpListener::bind(config.listen_addr)
        .await
        .context("Failed to bind listener")?;

    info!(
        "Proxy server listening on {} -> {}",
        config.listen_addr, config.target_addr
    );

    info!(
        "Configuration: max_connection={}, timeout={:?}, buffer_size={}",
        config.max_connection, config.timeout, config.buffer_size
    );

    // Connection limiter
    let semaphore = Arc::new(Semaphore::new(config.max_connection));

    let metric_config = config.clone();
    tokio::spawn(async move {
        log_metrics(metric_config.metrics_interval).await;
    });

    let mut connection_id = 0u64;

    loop {
        match listener.accept().await {
            Ok((client_stream, client_addr)) => {
                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        warn!(
                            "Connection limit reached, rejecting connection from {}",
                            client_addr
                        );
                        continue;
                    }
                };

                connection_id += 1;
                ACTIVE_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
                TOTAL_CONNECTIONS.fetch_add(1, Ordering::Relaxed);

                let config_clone = config.clone();
                let connection_id_clone = connection_id;

                tokio::spawn(async move {
                    let _guard = guard((), |_| {
                        ACTIVE_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
                    });

                    if let Err(e) =
                        handle_connection(client_stream, config_clone, connection_id_clone, permit)
                            .await
                    {
                        error!("Connection {} error: {}", connection_id_clone, e);
                    }
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {}", e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    env_logger::Builder::new()
        .filter_level(cli.log_level.clone().into())
        .format_timestamp_secs()
        .init();

    let config = ProxyConfig::from_cli(cli);

    info!("Starting async TCP proxy server...");

    // Handle graceful shutdown
    let shutdown_signal = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
        info!("Received shutdown signal, gracefully shutting down...");
    };

    tokio::select! {
        result = run_proxy(config) => {
            if let Err(e) = result {
                error!("Proxy server error: {}", e);
                std::process::exit(1);
            }
        }
        _ = shutdown_signal => {
            info!("Shutdown complete");
        }
    }

    Ok(())
}
