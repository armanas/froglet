use crate::{config::PaymentBackend, state::AppState};
use futures::future::BoxFuture;

use super::{
    PaymentError, PaymentReceipt, PaymentReservation, PreparePaymentRequest, SettlementDriver,
    SettlementDriverDescriptor, WalletBalanceSnapshot,
};

pub(crate) struct NoSettlementDriver;

impl SettlementDriver for NoSettlementDriver {
    fn descriptor(&self, _state: &AppState) -> SettlementDriverDescriptor {
        SettlementDriverDescriptor {
            backend: PaymentBackend::None.to_string(),
            mode: "disabled".to_string(),
            accepted_payment_methods: Vec::new(),
            capabilities: Vec::new(),
            reservations: false,
            receipts: false,
        }
    }

    fn wallet_balance<'a>(
        &'a self,
        state: &'a AppState,
    ) -> BoxFuture<'a, Result<WalletBalanceSnapshot, PaymentError>> {
        Box::pin(async move {
            Ok(WalletBalanceSnapshot::from_descriptor(
                self.descriptor(state),
            ))
        })
    }

    fn prepare<'a>(
        &'a self,
        _state: &'a AppState,
        request: PreparePaymentRequest,
    ) -> BoxFuture<'a, Result<Option<PaymentReservation>, PaymentError>> {
        Box::pin(async move {
            if request.price_sats == 0 {
                return Ok(None);
            }

            Err(PaymentError::BackendUnavailable {
                service_id: request.service_id.as_str().to_string(),
                price_sats: request.price_sats,
                backend: PaymentBackend::None.to_string(),
            })
        })
    }

    fn commit<'a>(
        &'a self,
        _state: &'a AppState,
        _reservation: PaymentReservation,
    ) -> BoxFuture<'a, Result<PaymentReceipt, PaymentError>> {
        Box::pin(async move {
            Err(PaymentError::BackendUnavailable {
                service_id: "unknown".to_string(),
                price_sats: 0,
                backend: PaymentBackend::None.to_string(),
            })
        })
    }

    fn release<'a>(
        &'a self,
        _state: &'a AppState,
        _reservation: &'a PaymentReservation,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { Ok(()) })
    }
}

pub(crate) static NO_SETTLEMENT_DRIVER: NoSettlementDriver = NoSettlementDriver;
