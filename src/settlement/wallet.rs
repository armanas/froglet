use crate::lnd::{CreatedInvoice, InvoiceDetails, LndNodeInfo, LndRestError};
use futures::future::BoxFuture;
use std::sync::Arc;
use thiserror::Error;

/// Backend-neutral error type for the [`LightningWallet`] trait.
///
/// Concrete backends (LndRest, LDK, …) convert their internal errors into
/// this enum so that call sites in `settlement/lightning.rs` do not depend on
/// `LndRestError` directly.
#[derive(Debug, Error)]
pub enum WalletError {
    #[error("invoice not found")]
    NotFound,
    #[error("invoice already settled")]
    AlreadySettled,
    #[error("wallet backend error: {0}")]
    Backend(String),
}

/// Map an [`LndRestError`] to a [`WalletError`].
///
/// HTTP 404 → `NotFound`, HTTP 409 → `AlreadySettled`, everything else →
/// `Backend`.
pub(crate) fn map_lnd_error(error: LndRestError) -> WalletError {
    match error {
        LndRestError::Status { status: 404, .. } => WalletError::NotFound,
        LndRestError::Status { status: 409, .. } => WalletError::AlreadySettled,
        other => WalletError::Backend(other.to_string()),
    }
}

/// Abstraction over a Lightning wallet backend.
///
/// The trait exposes exactly the operations that
/// `settlement/lightning.rs` needs so that the settlement logic is
/// independent of the concrete backend (LND REST today, LDK in the
/// future).
///
/// All methods return [`BoxFuture`] so the trait object is `dyn`-safe
/// without an extra crate dependency, consistent with the existing
/// [`crate::settlement::SettlementDriver`] trait.
pub trait LightningWallet: Send + Sync {
    /// Return node identity information (pubkey, alias, version).
    fn get_info<'a>(&'a self) -> BoxFuture<'a, Result<LndNodeInfo, WalletError>>;

    /// Create a standard (non-hold) invoice.
    fn add_invoice<'a>(
        &'a self,
        value_msat: u64,
        expiry_secs: u64,
        memo: &'a str,
        private: bool,
    ) -> BoxFuture<'a, Result<CreatedInvoice, WalletError>>;

    /// Create a hold invoice bound to the given payment hash.
    fn add_hold_invoice<'a>(
        &'a self,
        payment_hash_hex: &'a str,
        value_msat: u64,
        expiry_secs: u64,
        cltv_expiry: u32,
        memo: &'a str,
        private: bool,
    ) -> BoxFuture<'a, Result<CreatedInvoice, WalletError>>;

    /// Settle a held invoice by revealing the preimage.
    ///
    /// Idempotent: returns `Ok(())` or `Err(WalletError::AlreadySettled)`
    /// when the invoice was already settled (mapped from LND HTTP 409).
    fn settle_invoice<'a>(
        &'a self,
        preimage_hex: &'a str,
    ) -> BoxFuture<'a, Result<(), WalletError>>;

    /// Cancel / fail a held invoice by payment hash.
    fn cancel_invoice<'a>(
        &'a self,
        payment_hash_hex: &'a str,
    ) -> BoxFuture<'a, Result<(), WalletError>>;

    /// Look up the current state of an invoice by payment hash.
    fn lookup_invoice<'a>(
        &'a self,
        payment_hash_hex: &'a str,
    ) -> BoxFuture<'a, Result<InvoiceDetails, WalletError>>;
}

// ─── LndRestClient implementation ────────────────────────────────────────────

impl LightningWallet for crate::lnd::LndRestClient {
    fn get_info<'a>(&'a self) -> BoxFuture<'a, Result<LndNodeInfo, WalletError>> {
        Box::pin(async move { self.get_info().await.map_err(map_lnd_error) })
    }

    fn add_invoice<'a>(
        &'a self,
        value_msat: u64,
        expiry_secs: u64,
        memo: &'a str,
        private: bool,
    ) -> BoxFuture<'a, Result<CreatedInvoice, WalletError>> {
        Box::pin(async move {
            self.add_invoice(value_msat, expiry_secs, memo, private)
                .await
                .map_err(map_lnd_error)
        })
    }

    fn add_hold_invoice<'a>(
        &'a self,
        payment_hash_hex: &'a str,
        value_msat: u64,
        expiry_secs: u64,
        cltv_expiry: u32,
        memo: &'a str,
        private: bool,
    ) -> BoxFuture<'a, Result<CreatedInvoice, WalletError>> {
        Box::pin(async move {
            self.add_hold_invoice(
                payment_hash_hex,
                value_msat,
                expiry_secs,
                cltv_expiry,
                memo,
                private,
            )
            .await
            .map_err(map_lnd_error)
        })
    }

    fn settle_invoice<'a>(
        &'a self,
        preimage_hex: &'a str,
    ) -> BoxFuture<'a, Result<(), WalletError>> {
        Box::pin(async move {
            self.settle_invoice(preimage_hex)
                .await
                .map_err(map_lnd_error)
        })
    }

    fn cancel_invoice<'a>(
        &'a self,
        payment_hash_hex: &'a str,
    ) -> BoxFuture<'a, Result<(), WalletError>> {
        Box::pin(async move {
            self.cancel_invoice(payment_hash_hex)
                .await
                .map_err(map_lnd_error)
        })
    }

    fn lookup_invoice<'a>(
        &'a self,
        payment_hash_hex: &'a str,
    ) -> BoxFuture<'a, Result<InvoiceDetails, WalletError>> {
        Box::pin(async move {
            self.lookup_invoice(payment_hash_hex)
                .await
                .map_err(map_lnd_error)
        })
    }
}

/// Convenience alias: a trait object wrapped in `Arc`.
pub type ArcLightningWallet = Arc<dyn LightningWallet>;
