// Shortwave - client.rs
// Copyright (C) 2021-2025  Felix Häcker <haeckerfelix@gnome.org>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::net::IpAddr;
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::Duration;

use async_compat::Compat;
use async_io::Timer;
use async_std_resolver::{config as rconfig, resolver, resolver_from_system_conf};
use rand::prelude::SliceRandom;
use rand::thread_rng;
use rand::Rng;
use reqwest::header::{self, HeaderMap};
use reqwest::Method;
use serde::de;
use url::Url;

use crate::api::*;
use crate::app::SwApplication;
use crate::config;
use crate::settings::{settings_manager, Key};

static USER_AGENT: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}/{}-{}",
        config::PKGNAME,
        config::VERSION,
        config::PROFILE
    )
});

// Known working RadioBrowser fallback servers used when DNS discovery fails
// on unstable networks.
const FALLBACK_SERVERS: &[&str] = &[
    "de1.api.radio-browser.info",
    "de2.api.radio-browser.info",
    "at1.api.radio-browser.info",
    "nl1.api.radio-browser.info",
];

// Base delay (ms) for exponential backoff between retries
const BASE_RETRY_DELAY_MS: u64 = 1000;

// Maximum delay (ms) for exponential backoff
const MAX_RETRY_DELAY_MS: u64 = 8_000;

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        header::HeaderValue::from_static("application/json"),
    );

    reqwest::ClientBuilder::new()
        .user_agent(USER_AGENT.as_str())
        .default_headers(headers)
        // Generous default timeout; per-request timeouts in the retry loop
        // provide tighter control for individual requests.
        .timeout(Duration::from_secs(60))
        // Separate connect timeout so a slow TCP handshake doesn't block
        // the entire request window.
        .connect_timeout(Duration::from_secs(10))
        // TCP keepalive to detect dead connections faster on unstable networks.
        .tcp_keepalive(Duration::from_secs(15))
        .build()
        .unwrap()
});

pub async fn station_request(request: StationRequest) -> Result<Vec<SwStation>, Error> {
    let url = build_url(STATION_SEARCH, Some(&request.url_encode()))?;

    let stations_md = send_request_compat::<Vec<StationMetadata>>(Method::GET, url, None).await?;

    let stations: Vec<SwStation> = stations_md
        .into_iter()
        .map(|metadata| SwStation::new(&metadata.stationuuid.clone(), false, metadata, None))
        .collect();

    Ok(stations)
}

pub async fn station_metadata_by_uuid(uuids: Vec<String>) -> Result<Vec<StationMetadata>, Error> {
    let url = build_url(STATION_BY_UUID, None)?;

    let body = format!(
        r#"{{"uuids":{}}}"#,
        serde_json::to_string(&uuids).unwrap_or_default()
    );
    debug!("Post body: {}", body);

    send_request_compat(Method::POST, url, Some(body)).await
}

pub async fn lookup_rb_server() -> Option<String> {
    // Try DNS-based discovery first (the standard RadioBrowser approach)
    if let Some(server) = lookup_via_dns().await {
        return Some(server);
    }

    // DNS discovery failed; try the well-known fallback servers directly.
    // This helps on unstable networks where DNS resolution is unreliable.
    warn!(
        "DNS discovery failed, trying {} fallback servers",
        FALLBACK_SERVERS.len()
    );
    lookup_via_fallback().await
}

async fn lookup_via_dns() -> Option<String> {
    let lookup_domain = settings_manager::string(Key::ApiLookupDomain);

    let resolver = if let Ok(resolver) = resolver_from_system_conf().await {
        resolver
    } else {
        warn!("Unable to use dns resolver from system conf");

        let config = rconfig::ResolverConfig::default();
        let opts = rconfig::ResolverOpts::default();
        resolver(config, opts).await
    };

    // Do forward lookup to receive a list with the api servers
    let response = resolver.lookup_ip(&lookup_domain).await.ok()?;
    let mut ips: Vec<IpAddr> = response.iter().collect();

    if ips.is_empty() {
        warn!("DNS lookup for {} returned no addresses", lookup_domain);
        return None;
    }

    // Shuffle to distribute load across servers
    ips.shuffle(&mut thread_rng());

    for ip in ips {
        // Do a reverse lookup to get the hostname
        let result = resolver
            .reverse_lookup(ip)
            .await
            .ok()
            .and_then(|r| r.into_iter().next());

        if result.is_none() {
            warn!("Reverse lookup for {} failed", ip);
            continue;
        }

        // Strip trailing "." from domain name, otherwise TLS hostname verification fails
        let domain = result.unwrap().to_string();
        let hostname = domain.trim_end_matches(".");

        // Check if the server is online / returns data
        debug!("Trying to connect to {} ({})", hostname, ip);
        match server_stats(hostname).await {
            Ok(stats) => {
                debug!(
                    "Successfully connected to {} ({}), server version {}, {} stations",
                    hostname, ip, stats.software_version, stats.stations
                );
                return Some(format!("https://{hostname}/"));
            }
            Err(err) => warn!("Unable to connect to {hostname}: {}", err),
        }
    }

    None
}

async fn lookup_via_fallback() -> Option<String> {
    let mut servers: Vec<&&str> = FALLBACK_SERVERS.iter().collect();
    servers.shuffle(&mut thread_rng());

    for hostname in servers {
        debug!("Trying fallback server: {}", hostname);
        match server_stats(hostname).await {
            Ok(stats) => {
                info!(
                    "Successfully connected to fallback {} (server version {}, {} stations)",
                    hostname, stats.software_version, stats.stations
                );
                return Some(format!("https://{hostname}/"));
            }
            Err(err) => warn!("Unable to connect to fallback server {hostname}: {}", err),
        }
    }

    None
}

fn build_url(param: &str, options: Option<&str>) -> Result<Url, Error> {
    let rb_server = SwApplication::default().rb_server();
    if rb_server.is_none() {
        return Err(Error::NoServerAvailable);
    }

    let mut url = Url::parse(&rb_server.unwrap())
        .expect("Unable to parse server url")
        .join(param)
        .expect("Unable to join url");

    if let Some(options) = options {
        url.set_query(Some(options))
    }

    debug!("Retrieve data: {}", url);
    Ok(url)
}

async fn server_stats(host: &str) -> Result<Stats, Error> {
    let url =
        Url::parse(&format!("https://{host}/{STATS}")).expect("Unable to parse server stats url");

    send_request_compat(Method::GET, url, None).await
}

/// Execute an HTTP request and deserialize the JSON response.
/// Retries on transient errors (timeouts, connection failures, DNS errors)
/// with exponential backoff and jitter.
///
/// The number of retries and per-request timeout are read from GSettings,
/// allowing users on unstable networks to tune the behaviour.
async fn send_request<T: de::DeserializeOwned>(
    method: Method,
    url: Url,
    body: Option<String>,
) -> Result<T, Error> {
    let retry_count = {
        let configured = settings_manager::integer(Key::ApiRetryCount);
        if configured > 0 {
            configured as u32
        } else {
            0
        }
    };

    let per_request_timeout = {
        let configured = settings_manager::integer(Key::ApiTimeout);
        if configured > 0 {
            Duration::from_secs(configured as u64)
        } else {
            Duration::from_secs(30)
        }
    };

    let mut last_error: Option<Error> = None;
    let mut last_status: Option<reqwest::StatusCode> = None;

    for attempt in 0..=retry_count {
        // Build a fresh request each attempt so timeouts are per-attempt
        let mut req_builder = HTTP_CLIENT
            .request(method.clone(), url.clone())
            .timeout(per_request_timeout);
        if let Some(ref body) = body {
            req_builder = req_builder.body(body.clone());
        }

        let request = match req_builder.build() {
            Ok(req) => req,
            Err(err) => {
                error!("Failed to build request: {}", err);
                return Err(Error::Network(Rc::new(err)));
            }
        };

        match HTTP_CLIENT.execute(request).await {
            Ok(response) => {
                let status = response.status();
                let json = match response.text().await {
                    Ok(t) => t,
                    Err(err) => {
                        if attempt < retry_count && is_transient_http_err(&err) {
                            last_error = Some(Error::Network(Rc::new(err)));
                            backoff_delay(attempt).await;
                            continue;
                        }
                        return Err(Error::Network(Rc::new(err)));
                    }
                };

                // Retry on server errors (5xx) that may be transient
                if status.is_server_error() {
                    if attempt < retry_count {
                        warn!(
                            "Server returned {} (attempt {}/{}), retrying...",
                            status.as_u16(),
                            attempt + 1,
                            retry_count
                        );
                        last_status = Some(status);
                        backoff_delay(attempt).await;
                        continue;
                    }
                    return Err(Error::RetryExhausted(format!(
                        "Server returned {} after {} retries",
                        status.as_u16(),
                        retry_count
                    )));
                }

                match serde_json::from_str(&json) {
                    Ok(d) => return Ok(d),
                    Err(err) => {
                        error!("Unable to deserialize data: {}", err);
                        error!("Raw unserialized data: {}", json);
                        return Err(Error::Deserializer(err.into()));
                    }
                }
            }
            Err(err) => {
                if attempt < retry_count && is_transient_http_err(&err) {
                    warn!(
                        "Request failed (attempt {}/{}): {}. Retrying...",
                        attempt + 1,
                        retry_count,
                        err
                    );
                    last_error = Some(Error::Network(Rc::new(err)));
                    backoff_delay(attempt).await;
                    continue;
                }
                return Err(Error::Network(Rc::new(err)));
            }
        }
    }

    // All retries exhausted — return the last error or status
    Err(match last_status {
        Some(status) => Error::RetryExhausted(format!(
            "Server returned {} after {} retries",
            status.as_u16(),
            retry_count
        )),
        None => last_error.unwrap_or_else(|| {
            Error::RetryExhausted("All retry attempts failed with no specific error".into())
        }),
    })
}

/// Check if an HTTP error is transient (worth retrying).
fn is_transient_http_err(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

/// Exponential backoff with jitter.
/// Waits: ~1s, ~2s, ~4s, ~8s, ... capped at MAX_RETRY_DELAY_MS.
async fn backoff_delay(attempt: u32) {
    let delay_ms = std::cmp::min(BASE_RETRY_DELAY_MS * 2u64.pow(attempt), MAX_RETRY_DELAY_MS);
    // Add ±25% jitter to avoid thundering herd
    let jitter = rand::thread_rng().gen_range(0..=delay_ms / 2);
    let jittered = delay_ms - delay_ms / 4 + jitter;

    debug!("Backoff: waiting {}ms after attempt {}", jittered, attempt);

    Timer::after(Duration::from_millis(jittered)).await;
}

/// Compatibility wrapper: runs `send_request` inside an async-compat context
/// so it can be called from GLib's main loop.
async fn send_request_compat<T: de::DeserializeOwned>(
    method: Method,
    url: Url,
    body: Option<String>,
) -> Result<T, Error> {
    Compat::new(async move { send_request(method, url, body).await }).await
}
