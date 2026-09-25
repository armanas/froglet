//! Relay endpoint validation.
//!
//! Planning reads the daemon's deterministic reserved endpoint. Activation is
//! deliberately not a generic `HostingBackend::prepare()` operation: it must
//! carry the exact persisted service/revision/activation token and therefore
//! lives on `DaemonClient::activate_relay_transport`.

use crate::error::PublishError;
use url::Url;

pub(crate) fn validated_public_relay_url(value: &str) -> Result<String, PublishError> {
    let url = Url::parse(value).map_err(|error| PublishError::Hosting {
        backend: "relay",
        reason: format!("daemon advertised an invalid relay URL: {error}"),
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PublishError::Hosting {
            backend: "relay",
            reason: "daemon relay URL must be a credential-free HTTPS origin".to_string(),
        });
    }
    Ok(url.to_string().trim_end_matches('/').to_string())
}

pub(crate) fn validated_relay_control_url(value: &str) -> Result<String, PublishError> {
    let url = Url::parse(value).map_err(|error| PublishError::Hosting {
        backend: "relay",
        reason: format!("daemon advertised an invalid relay control URL: {error}"),
    })?;
    let plaintext_loopback = url.scheme() == "ws"
        && url.host().is_some_and(|host| match host {
            url::Host::Domain(domain) => domain == "localhost",
            url::Host::Ipv4(address) => address.is_loopback(),
            url::Host::Ipv6(address) => address.is_loopback(),
        });
    if (url.scheme() != "wss" && !plaintext_loopback)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PublishError::Hosting {
            backend: "relay",
            reason: "daemon relay control URL must be credential-free WSS (or loopback WS) without query or fragment"
                .to_string(),
        });
    }
    Ok(value.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_url_must_be_https_origin() {
        assert!(validated_public_relay_url("https://relay.example/").is_ok());
        assert!(validated_public_relay_url("http://relay.example").is_err());
        assert!(validated_public_relay_url("https://user@relay.example").is_err());
        assert!(validated_public_relay_url("https://relay.example/api").is_err());
        assert!(validated_public_relay_url("https://relay.example/?x=1").is_err());
    }

    #[test]
    fn control_url_must_be_wss_or_loopback_ws() {
        assert!(validated_relay_control_url("wss://control.example/v1/tunnel").is_ok());
        assert!(validated_relay_control_url("ws://127.0.0.1:9000/v1/tunnel").is_ok());
        assert!(validated_relay_control_url("ws://control.example/v1/tunnel").is_err());
        assert!(validated_relay_control_url("wss://user@control.example/tunnel").is_err());
        assert!(validated_relay_control_url("wss://control.example/tunnel?token=x").is_err());
    }
}
