pub mod config;
pub mod db;
pub mod handlers;
pub mod indexer;
pub mod verify;

use config::MarketplaceConfig;
use froglet::execution::BuiltinServiceHandler;
use handlers::{
    provider::MarketplaceProviderHandler, receipts::MarketplaceReceiptsHandler,
    register::MarketplaceRegisterHandler, search::MarketplaceSearchHandler,
    stake::MarketplaceStakeHandler, topup::MarketplaceTopupHandler,
};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{error, info};

const MARKETPLACE_SERVICES: &[(&str, &str)] = &[
    (
        "marketplace.register",
        "Register a provider with the marketplace",
    ),
    ("marketplace.search", "Search providers and offers"),
    ("marketplace.provider", "Get provider details and stake"),
    ("marketplace.receipts", "Get provider receipts"),
    ("marketplace.stake", "Stake into provider identity"),
    ("marketplace.topup", "Top up provider identity stake"),
];

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    froglet::init_logging();
    froglet::tls::ensure_rustls_crypto_provider();

    println!("\n=========================================");
    println!(" 🐸 Froglet Marketplace is Starting...");
    println!("=========================================\n");

    // Load node config (standard Froglet provider config)
    let node_config = froglet::config::NodeConfig::from_env().map_err(|e| {
        error!("{e}");
        e
    })?;

    // Load marketplace-specific config
    let marketplace_config = MarketplaceConfig::from_env().map_err(|e| {
        error!("{e}");
        e
    })?;

    // Connect to Postgres
    info!("Connecting to marketplace database...");
    let pg_pool = db::connect(&marketplace_config.database_url).await?;

    // Run migrations
    info!("Running database migrations...");
    db::run_migrations(&pg_pool).await?;

    let pg = Arc::new(pg_pool);

    // Build marketplace service handlers
    let builtin_services = build_service_handlers(pg.clone());

    // Build standard Froglet AppState
    let state = froglet::state::build_app_state(node_config)?;

    // Inject builtin services and event batch writer via Arc::get_mut
    // (same pattern as server.rs — safe because no clones exist yet)
    let state = {
        let mut state = state;
        {
            let inner = Arc::get_mut(&mut state).expect("no other Arc references at startup");
            inner.builtin_services = builtin_services;
            let db_clone = inner.db.clone();
            inner.event_batch_writer = Some(froglet::db::EventBatchWriter::spawn(db_clone));
        }
        state
    };

    info!("Node identity: {}", state.identity.node_id());

    // Auto-register marketplace offer definitions
    register_marketplace_offers(&state).await?;

    // Spawn the indexer background task
    let indexer_pg = pg.clone();
    let indexer_config = marketplace_config.clone();
    let indexer_http = state.http_client.clone();
    tokio::spawn(async move {
        indexer::run(indexer_pg, indexer_config, indexer_http).await;
    });
    info!(
        "Indexer started with {} feed source(s)",
        marketplace_config.feed_sources.len()
    );

    info!("Marketplace provider ready — queries are served through Froglet deals");
    let service_list: Vec<&str> = MARKETPLACE_SERVICES.iter().map(|(id, _)| *id).collect();
    info!("Service kinds: {}", service_list.join(", "));

    // Start the server with our custom state (handlers injected)
    froglet::server::run_with_state(state).await?;

    Ok(())
}

fn build_service_handlers(pg: Arc<db::PgPool>) -> HashMap<String, Arc<dyn BuiltinServiceHandler>> {
    let mut handlers: HashMap<String, Arc<dyn BuiltinServiceHandler>> = HashMap::new();
    handlers.insert(
        "marketplace.register".to_string(),
        Arc::new(MarketplaceRegisterHandler { pg: pg.clone() }),
    );
    handlers.insert(
        "marketplace.search".to_string(),
        Arc::new(MarketplaceSearchHandler { pg: pg.clone() }),
    );
    handlers.insert(
        "marketplace.provider".to_string(),
        Arc::new(MarketplaceProviderHandler { pg: pg.clone() }),
    );
    handlers.insert(
        "marketplace.receipts".to_string(),
        Arc::new(MarketplaceReceiptsHandler { pg: pg.clone() }),
    );
    handlers.insert(
        "marketplace.stake".to_string(),
        Arc::new(MarketplaceStakeHandler { pg: pg.clone() }),
    );
    handlers.insert(
        "marketplace.topup".to_string(),
        Arc::new(MarketplaceTopupHandler { pg }),
    );
    handlers
}

async fn register_marketplace_offers(
    state: &Arc<froglet::state::AppState>,
) -> Result<(), Box<dyn std::error::Error>> {
    for (service_id, summary) in MARKETPLACE_SERVICES {
        let request = froglet::api::ProviderControlPublishArtifactRequest {
            service_id: service_id.to_string(),
            offer_id: Some(service_id.to_string()),
            artifact_path: None,
            wasm_module_hex: None,
            oci_reference: None,
            oci_digest: None,
            runtime: Some("builtin".to_string()),
            package_kind: Some("builtin".to_string()),
            entrypoint_kind: Some("builtin".to_string()),
            entrypoint: Some(service_id.to_string()),
            contract_version: Some(format!("froglet.builtin.{service_id}.v1")),
            mounts: None,
            inline_source: None,
            summary: Some(summary.to_string()),
            mode: None,
            price_sats: 0,
            publication_state: Some("active".to_string()),
            input_schema: None,
            output_schema: None,
        };

        let definition = froglet::api::artifact_provider_offer_definition(state.as_ref(), request)
            .map_err(|(status, body)| {
                format!("offer definition for {service_id}: {status} {body}")
            })?;

        let _response = froglet::api::persist_provider_offer_mutation(
            state.as_ref(),
            definition,
            axum::http::StatusCode::CREATED,
            format!("registered marketplace service {service_id}"),
        )
        .await
        .map_err(|(status, body)| format!("persist offer for {service_id}: {status} {body:?}"))?;

        info!("Registered marketplace offer: {service_id}");
    }

    Ok(())
}
