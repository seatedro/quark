//! Profiler startup and frame marks for the profile-puffin and
//! profile-tracy features.

/// Where the puffin server listens; connect with `puffin_viewer`.
#[cfg(feature = "profile-puffin")]
const PUFFIN_ADDR: &str = "127.0.0.1:8585";

/// Start the profiler once per process.
pub(crate) fn start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        #[cfg(feature = "profile-puffin")]
        {
            profiling::puffin::set_scopes_on(true);
            match puffin_http::Server::new(PUFFIN_ADDR) {
                // The server runs for the life of the process.
                Ok(server) => {
                    tracing::info!("puffin profiler serving on {PUFFIN_ADDR}");
                    std::mem::forget(server);
                }
                Err(error) => tracing::warn!("could not start the puffin server: {error}"),
            }
        }
        #[cfg(feature = "profile-tracy")]
        {
            // Spans panic without a running client.
            let _ = profiling::tracy_client::Client::start();
        }
    });
}

/// Mark the end of a frame for the profiler's frame view.
pub(crate) fn finish_frame() {
    profiling::finish_frame!();
}
