/// Run an async future on a fresh current-thread runtime.
/// Safe to call from `tokio::task::spawn_blocking` (not from a Tokio worker directly).
pub fn block_on_local<F>(future: F) -> F::Output
where
    F: std::future::Future,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("local runtime")
        .block_on(future)
}
