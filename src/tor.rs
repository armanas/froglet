use arti_client::{TorClient, TorClientConfig};
use axum::Router;
use futures::StreamExt;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use std::path::PathBuf;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tor_cell::relaycell::msg::Connected;
use tor_hsservice::config::OnionServiceConfigBuilder;
use tower::Service;

pub async fn start_hidden_service(app: Router, tor_dir: PathBuf) -> Result<String, String> {
    tracing::info!("Initializing native pure-Rust Tor (Arti) engine...");

    let state_dir = tor_dir.join("state");
    let cache_dir = tor_dir.join("cache");
    std::fs::create_dir_all(&state_dir)
        .map_err(|e| format!("failed to create Tor state directory {state_dir:?}: {e}"))?;
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("failed to create Tor cache directory {cache_dir:?}: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&state_dir)
            .map_err(|e| format!("failed to read metadata for Tor state directory {state_dir:?}: {e}"))?;
        let mut perms = metadata.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&state_dir, perms).map_err(|e| {
            format!(
                "failed to set permissions on Tor state directory {state_dir:?} to 0o700: {e}"
            )
        })?;

        let metadata = std::fs::metadata(&cache_dir)
            .map_err(|e| format!("failed to read metadata for Tor cache directory {cache_dir:?}: {e}"))?;
        let mut perms = metadata.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&cache_dir, perms).map_err(|e| {
            format!(
                "failed to set permissions on Tor cache directory {cache_dir:?} to 0o700: {e}"
            )
        })?;
    }

    let mut builder = TorClientConfig::builder();
    builder.storage().state_dir(tor_config::CfgPath::new(
        state_dir.to_string_lossy().to_string(),
    ));
    builder.storage().cache_dir(tor_config::CfgPath::new(
        cache_dir.to_string_lossy().to_string(),
    ));
    let config = builder.build().map_err(|e| e.to_string())?;
    let tor_client = TorClient::create_bootstrapped(config)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("Tor circuit bootstrapped securely!");

    let mut svc_config_builder = OnionServiceConfigBuilder::default();
    svc_config_builder.nickname(
        "froglet"
            .to_string()
            .try_into()
            .map_err(|e| format!("{e:?}"))?,
    );

    let svc_config = svc_config_builder.build().map_err(|e| format!("{e:?}"))?;
    let (onion_service, mut rendezvous) = tor_client
        .launch_onion_service(svc_config)
        .map_err(|e| e.to_string())?;

    let onion_id = onion_service
        .onion_name()
        .map(|name| name.to_string())
        .ok_or_else(|| "Failed to get onion service name".to_string())?;

    let onion_url = format!("http://{onion_id}");
    println!(" 🧅 Tor Hidden Service: {onion_url}");
    println!("\n=========================================");
    println!(" ✅ Node is now online and accepting traffic.");
    println!("=========================================\n");

    let onion_url_clone = onion_url.clone();

    tokio::spawn(async move {
        while let Some(rend_req) = rendezvous.next().await {
            let app_bound = app.clone();
            tokio::spawn(async move {
                match rend_req.accept().await {
                    Ok(mut rend_stream) => {
                        while let Some(stream_req) = rend_stream.next().await {
                            let app_clone = app_bound.clone();
                            tokio::spawn(async move {
                                match stream_req.accept(Connected::new_empty()).await {
                                    Ok(data_stream) => {
                                        let io = TokioIo::new(data_stream.compat());
                                        let app_router = app_clone.clone();

                                        let hyper_service = hyper::service::service_fn(
                                            move |req: hyper::Request<hyper::body::Incoming>| {
                                                app_router.clone().call(req)
                                            },
                                        );

                                        let mut builder = http1::Builder::new();
                                        builder.half_close(true);

                                        if let Err(e) =
                                            builder.serve_connection(io, hyper_service).await
                                        {
                                            tracing::error!(
                                                "Error serving Axum over Tor data stream: {}",
                                                e
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to accept virt data stream: {}", e)
                                    }
                                }
                            });
                        }
                    }
                    Err(e) => tracing::error!("Failed to accept rendezvous stream: {}", e),
                }
            });
        }
    });

    Ok(onion_url_clone)
}
