//! The core installs a TLS crypto provider when it starts. matrix-sdk and
//! LiveKit build rustls with two providers (aws-lc-rs and ring), so rustls
//! cannot pick one by itself. LiveKit's websocket (tokio-tungstenite) builds
//! its `ClientConfig` from the process default, and the first `wss://` call
//! connection panicked. This is a test binary of its own: the process starts
//! without a provider. No server is needed.

use std::sync::Arc;

use rustls::crypto::CryptoProvider;
use twicecat_core::api::Command;
use twicecat_core::paths::DataPaths;
use twicecat_core::secrets::InMemorySecretStore;
use twicecat_core::{Core, CoreConfig};

#[tokio::test]
async fn starting_the_core_installs_a_tls_provider() {
    assert!(
        CryptoProvider::get_default().is_none(),
        "no provider before the core starts"
    );
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start(
        CoreConfig {
            data: DataPaths::at(dir.path()),
            homeserver_override: None,
            room_list_page_size: twicecat_core::DEFAULT_ROOM_LIST_PAGE_SIZE,
            max_upload_bytes_override: None,
            audio: Default::default(),
            call_service_override: None,
            video: twicecat_core::api::VideoBackend::System,
        },
        Arc::new(InMemorySecretStore::new()),
    );
    assert!(CryptoProvider::get_default().is_some());
    // What tokio-tungstenite does for a `wss://` URL; it panicked before.
    let _ = rustls::ClientConfig::builder();
    core.commands.send(Command::Shutdown).await.unwrap();
}
