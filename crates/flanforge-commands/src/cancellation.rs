use tokio_util::sync::CancellationToken;

pub(crate) fn signal_token() -> CancellationToken {
    let token = CancellationToken::new();
    let signal = token.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            let Ok(mut terminate) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            else {
                let _ = tokio::signal::ctrl_c().await;
                signal.cancel();
                return;
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => signal.cancel(),
                _ = terminate.recv() => signal.cancel(),
            }
        }
        #[cfg(not(unix))]
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.cancel();
        }
    });
    token
}
