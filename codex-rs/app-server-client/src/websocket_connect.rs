use std::net::IpAddr;

use tokio::net::TcpStream;
use tokio_tungstenite::Connector;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::client_async_tls_with_config;
use tokio_tungstenite::connect_async_tls_with_config;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::error::UrlError;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use url::Url;

pub(crate) async fn connect_websocket_request(
    request: Request,
    url: &Url,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, WsError> {
    if !should_bypass_proxy(url) {
        return connect_async_tls_with_config(request, None, false, None)
            .await
            .map(|(stream, _response)| stream);
    }

    let host = request
        .uri()
        .host()
        .ok_or(WsError::Url(UrlError::NoHostName))?;
    let port = request
        .uri()
        .port_u16()
        .or_else(|| match request.uri().scheme_str() {
            Some("wss") => Some(443),
            Some("ws") => Some(80),
            _ => None,
        })
        .ok_or(WsError::Url(UrlError::UnsupportedUrlScheme))?;
    let address = match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{host}]:{port}"),
        Ok(IpAddr::V4(_)) | Err(_) => format!("{host}:{port}"),
    };
    let stream = TcpStream::connect(address).await.map_err(WsError::Io)?;
    client_async_tls_with_config(request, stream, None::<WebSocketConfig>, None::<Connector>)
        .await
        .map(|(stream, _response)| stream)
}

fn should_bypass_proxy(url: &Url) -> bool {
    url.host_str().is_some_and(is_loopback_host)
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::should_bypass_proxy;
    use url::Url;

    #[test]
    fn loopback_hosts_bypass_proxy() {
        assert!(should_bypass_proxy(
            &Url::parse("ws://localhost:3000").expect("localhost URL should parse")
        ));
        assert!(should_bypass_proxy(
            &Url::parse("ws://127.0.0.1:3000").expect("ipv4 URL should parse")
        ));
        assert!(should_bypass_proxy(
            &Url::parse("ws://[::1]:3000").expect("ipv6 URL should parse")
        ));
        assert!(!should_bypass_proxy(
            &Url::parse("ws://example.com:3000").expect("remote URL should parse")
        ));
    }
}
