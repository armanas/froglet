use crate::config::PricingConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceId {
    EventsQuery,
    ExecuteWasm,
}

impl ServiceId {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceId::EventsQuery => "events.query",
            ServiceId::ExecuteWasm => "execute.wasm",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePriceInfo {
    pub service_id: String,
    pub price_sats: u64,
    pub payment_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingInfo {
    pub events_query: ServicePriceInfo,
    pub execute_wasm: ServicePriceInfo,
}

#[derive(Debug, Clone)]
pub struct PricingTable {
    info: PricingInfo,
}

impl PricingTable {
    pub fn from_config(config: PricingConfig) -> Self {
        Self {
            info: PricingInfo {
                events_query: Self::entry(ServiceId::EventsQuery, config.events_query),
                execute_wasm: Self::entry(ServiceId::ExecuteWasm, config.execute_wasm),
            },
        }
    }

    pub fn info(&self) -> &PricingInfo {
        &self.info
    }

    pub fn price_for(&self, service: ServiceId) -> u64 {
        match service {
            ServiceId::EventsQuery => self.info.events_query.price_sats,
            ServiceId::ExecuteWasm => self.info.execute_wasm.price_sats,
        }
    }

    pub fn services(&self) -> Vec<ServicePriceInfo> {
        vec![
            self.info.events_query.clone(),
            self.info.execute_wasm.clone(),
        ]
    }

    fn entry(service: ServiceId, price_sats: u64) -> ServicePriceInfo {
        ServicePriceInfo {
            service_id: service.as_str().to_string(),
            price_sats,
            payment_required: price_sats > 0,
        }
    }
}
