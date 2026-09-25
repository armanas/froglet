//! Hosting backends for local, relay, Tor, and self-hosted reachability.
//! Provider-neutral managed deployment remains an operator-adapter seam.

pub mod local;
pub mod relay;
pub mod self_hosted;
pub mod tor;

use crate::error::PublishError;
use async_trait::async_trait;

/// Result of a backend's `prepare()` call: enough info to sign the
/// descriptor and (optionally) register with the marketplace.
#[derive(Debug, Clone)]
pub struct PreparedHosting {
    /// The public URL the service can be reached at.
    pub public_url: String,
    /// `true` if the backend wants the engine to register with the
    /// marketplace. `false` for Local (private).
    pub register_with_marketplace: bool,
    /// Endpoint-specific Revision when a backend must bind a different signed
    /// Descriptor than the locally verified authoring node. Managed hosting
    /// uses this to advertise only its approved public URL.
    pub publication_revision: Option<froglet_protocol::publication::SignedPublicationRevision>,
}

/// Each hosting backend implements this. The engine drives the pipeline
/// uniformly; backends own their specifics (transport supervision, URL
/// validation, managed deployment invocation, etc.).
#[async_trait]
pub trait HostingBackend: Send + Sync {
    /// Backend identifier for error messages.
    fn name(&self) -> &'static str;

    /// Bring the public surface online. Idempotent: repeated calls on
    /// the same backend instance must converge.
    async fn prepare(&self) -> Result<PreparedHosting, PublishError>;
}
