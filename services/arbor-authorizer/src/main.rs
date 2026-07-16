use arbor_authorizer::service::{AuthorizerService, ArborServer};
use tonic::transport::Server;

mod config;

use config::{AuthorizerConfig, Transport};

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let config = AuthorizerConfig::load().expect("failed to load configuration");

    let svc = AuthorizerService::load(&config.snapshot_path).expect("failed to load snapshot");
    let svc = ArborServer::new(svc);

    match config.transport {
        Transport::Uds => {
            let _ = std::fs::remove_file(&config.uds_path);
            let listener = tokio::net::UnixListener::bind(&config.uds_path)
                .expect("failed to bind UDS socket");
            let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener);

            println!("gRPC listening on unix:{}", config.uds_path.display());
            Server::builder()
                .max_concurrent_streams(Some(config.max_concurrent_streams))
                .add_service(svc)
                .serve_with_incoming_shutdown(incoming, shutdown_signal())
                .await
                .expect("UDS server error");
        }

        Transport::Tcp => {
            println!("gRPC listening on {}", config.grpc_addr);
            Server::builder()
                .max_concurrent_streams(Some(config.max_concurrent_streams))
                .add_service(svc)
                .serve_with_shutdown(config.grpc_addr, shutdown_signal())
                .await
                .expect("TCP server error");
        }

        Transport::Both => {
            let _ = std::fs::remove_file(&config.uds_path);
            let listener = tokio::net::UnixListener::bind(&config.uds_path)
                .expect("failed to bind UDS socket");
            let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener);

            println!("gRPC listening on {} and unix:{}", config.grpc_addr, config.uds_path.display());

            let uds_svc = svc.clone();
            let (uds_tx, uds_rx) = tokio::sync::oneshot::channel::<()>();
            let uds_max_streams = config.max_concurrent_streams;
            let uds_task = tokio::spawn(async move {
                Server::builder()
                    .max_concurrent_streams(Some(uds_max_streams))
                    .add_service(uds_svc)
                    .serve_with_incoming_shutdown(incoming, async {
                        let _ = uds_rx.await;
                    })
                    .await
                    .expect("UDS server error");
            });

            let (tcp_tx, tcp_rx) = tokio::sync::oneshot::channel::<()>();
            let tcp_max_streams = config.max_concurrent_streams;
            let tcp_task = tokio::spawn(async move {
                Server::builder()
                    .max_concurrent_streams(Some(tcp_max_streams))
                    .add_service(svc)
                    .serve_with_shutdown(config.grpc_addr, async {
                        let _ = tcp_rx.await;
                    })
                    .await
                    .expect("TCP server error");
            });

            shutdown_signal().await;
            println!("Shutting down gracefully.");

            let _ = uds_tx.send(());
            let _ = tcp_tx.send(());

            let _ = tokio::join!(uds_task, tcp_task);
            return;
        }
    }

    println!("Shutting down.");
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

    tokio::select! {
        _ = sigint.recv() => println!("Received SIGINT, shutting down."),
        _ = sigterm.recv() => println!("Received SIGTERM, shutting down."),
    }
}
