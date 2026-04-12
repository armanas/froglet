use axum::{Json, http::StatusCode};

use crate::{
    api::{
        self, ApiFailure, ProviderControlMutationResponse, ProviderControlPublishArtifactRequest,
        ProviderManagedOfferDefinition, ProviderServiceRecord,
    },
    state::AppState,
};

pub(crate) fn normalize_offer_publication_state(value: Option<&str>) -> Result<String, String> {
    api::normalize_offer_publication_state(value)
}

pub(crate) async fn current_service_records(
    state: &AppState,
    include_hidden: bool,
    include_binding: bool,
) -> Result<Vec<ProviderServiceRecord>, String> {
    api::current_service_records(state, include_hidden, include_binding).await
}

pub(crate) async fn provider_service_record(
    state: &AppState,
    service_id: &str,
    include_hidden: bool,
    include_binding: bool,
) -> Result<Option<ProviderServiceRecord>, String> {
    api::provider_service_record(state, service_id, include_hidden, include_binding).await
}

pub(crate) fn artifact_provider_offer_definition(
    state: &AppState,
    payload: ProviderControlPublishArtifactRequest,
) -> Result<ProviderManagedOfferDefinition, ApiFailure> {
    api::artifact_provider_offer_definition(state, payload)
}

pub(crate) async fn persist_provider_offer_mutation(
    state: &AppState,
    definition: ProviderManagedOfferDefinition,
    status_code: StatusCode,
    summary: String,
) -> Result<(StatusCode, Json<ProviderControlMutationResponse>), ApiFailure> {
    api::persist_provider_offer_mutation(state, definition, status_code, summary).await
}
