//! Graceful-shutdown signal handling. Listens for SIGTERM (k8s pod
//! termination) and Ctrl-C so axum can drain in-flight requests during a
//! Helm rollout instead of dropping them mid-flight.

pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install ctrl-c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => tracing::info!(signal = "SIGINT", "shutdown signal received"),
        _ = terminate => tracing::info!(signal = "SIGTERM", "shutdown signal received"),
    }
}
