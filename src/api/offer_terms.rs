//! Application-layer payment terms shared by discovery and publication.
//! Kernel artifact construction consumes these resolved terms unchanged.

use super::{
    AppState, LightningMode, PaymentBackend, ProviderManagedOfferDefinition, PublicationSettlement,
};

pub(super) fn provider_offer_price_schedule(
    definition: &ProviderManagedOfferDefinition,
) -> (u64, u64) {
    (
        definition.base_fee_msat.unwrap_or(0),
        definition
            .success_fee_msat
            .unwrap_or_else(|| definition.price_sats.saturating_mul(1_000)),
    )
}

pub(super) fn provider_offer_total_minor_units(definition: &ProviderManagedOfferDefinition) -> u64 {
    let (base_fee_msat, success_fee_msat) = provider_offer_price_schedule(definition);
    base_fee_msat.saturating_add(success_fee_msat) / 1_000
}

pub(super) fn kernel_paid_settlement_available(state: &AppState) -> bool {
    state
        .config
        .payment_backends
        .iter()
        .any(|backend| matches!(backend, PaymentBackend::Lightning | PaymentBackend::Stripe))
}

pub(super) fn provider_offer_definition_is_paid(
    definition: &ProviderManagedOfferDefinition,
) -> bool {
    let (base_fee_msat, success_fee_msat) = provider_offer_price_schedule(definition);
    base_fee_msat != 0 || success_fee_msat != 0
}

pub(super) fn provider_offer_settlement_method(
    state: &AppState,
    definition: &ProviderManagedOfferDefinition,
    base_fee_msat: u64,
    success_fee_msat: u64,
) -> String {
    resolved_offer_settlement_method(
        state,
        definition.settlement_method,
        base_fee_msat,
        success_fee_msat,
    )
}

pub(super) fn resolved_offer_settlement_method(
    state: &AppState,
    settlement_method: Option<PublicationSettlement>,
    base_fee_msat: u64,
    success_fee_msat: u64,
) -> String {
    if base_fee_msat == 0 && success_fee_msat == 0 {
        return "none".to_string();
    }
    match settlement_method {
        Some(PublicationSettlement::None) => "none".to_string(),
        Some(PublicationSettlement::Stripe) => "stripe_mpp.v1".to_string(),
        Some(PublicationSettlement::Lightning) => {
            if state.config.lightning.mode == LightningMode::Phoenixd {
                "lightning.prepaid.v1".to_string()
            } else {
                "lightning.base_fee_plus_success_fee.v1".to_string()
            }
        }
        None if state
            .config
            .payment_backends
            .contains(&PaymentBackend::Stripe)
            && !state
                .config
                .payment_backends
                .contains(&PaymentBackend::Lightning) =>
        {
            "stripe_mpp.v1".to_string()
        }
        None if state
            .config
            .payment_backends
            .contains(&PaymentBackend::Lightning)
            && state.config.lightning.mode == LightningMode::Phoenixd =>
        {
            "lightning.prepaid.v1".to_string()
        }
        None => "lightning.base_fee_plus_success_fee.v1".to_string(),
    }
}

pub(super) fn resolved_provider_offer_terms(
    state: &AppState,
    definition: &ProviderManagedOfferDefinition,
) -> (String, u64, u64) {
    let (scheduled_base_fee_msat, scheduled_success_fee_msat) =
        provider_offer_price_schedule(definition);
    let settlement_method = provider_offer_settlement_method(
        state,
        definition,
        scheduled_base_fee_msat,
        scheduled_success_fee_msat,
    );
    let (base_fee_msat, success_fee_msat) = if settlement_method == "none"
        || settlement_method == "lightning.base_fee_plus_success_fee.v1"
    {
        (scheduled_base_fee_msat, scheduled_success_fee_msat)
    } else {
        // Stripe and prepaid Lightning are canonical single-leg methods.
        // Invalid overflow is represented as the maximum value and rejected
        // by the checked Quote admission path instead of wrapping to free.
        (
            scheduled_base_fee_msat.saturating_add(scheduled_success_fee_msat),
            0,
        )
    };
    (settlement_method, base_fee_msat, success_fee_msat)
}
