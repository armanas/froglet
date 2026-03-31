use axum::{Json, http::StatusCode};

use crate::{
    api::{
        self, ApiFailure, ProviderControlMutationResponse, ProviderControlPublishArtifactRequest,
        ProviderManagedOfferDefinition, ProviderServiceRecord,
    },
    state::AppState,
};

pub fn normalize_offer_publication_state(value: Option<&str>) -> Result<String, String> {
    api::normalize_offer_publication_state(value)
}

pub async fn current_service_records(
    state: &AppState,
    include_hidden: bool,
    include_binding: bool,
) -> Result<Vec<ProviderServiceRecord>, String> {
    api::current_service_records(state, include_hidden, include_binding).await
}

pub async fn provider_service_record(
    state: &AppState,
    service_id: &str,
    include_hidden: bool,
    include_binding: bool,
) -> Result<Option<ProviderServiceRecord>, String> {
    api::provider_service_record(state, service_id, include_hidden, include_binding).await
}

pub fn artifact_provider_offer_definition(
    state: &AppState,
    payload: ProviderControlPublishArtifactRequest,
) -> Result<ProviderManagedOfferDefinition, ApiFailure> {
    api::artifact_provider_offer_definition(state, payload)
}

pub async fn persist_provider_offer_mutation(
    state: &AppState,
    definition: ProviderManagedOfferDefinition,
    status_code: StatusCode,
    summary: String,
) -> Result<(StatusCode, Json<ProviderControlMutationResponse>), ApiFailure> {
    api::persist_provider_offer_mutation(state, definition, status_code, summary).await
}
