use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Tencent Cloud API v3 client with TC3-HMAC-SHA256 signing.
pub struct TcClient {
    secret_id: String,
    secret_key: String,
    service: String,
    host: String,
    region: String,
    http: Client,
}

/// Generic Tencent Cloud API response envelope.
#[derive(Deserialize)]
#[allow(dead_code, non_snake_case)]
struct TcResponse {
    #[serde(default)]
    Response: serde_json::Value,
}

impl TcClient {
    pub fn new(
        secret_id: &str,
        secret_key: &str,
        service: &str,
        host: &str,
        region: &str,
    ) -> Result<Self> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            secret_id: secret_id.to_string(),
            secret_key: secret_key.to_string(),
            service: service.to_string(),
            host: host.to_string(),
            region: region.to_string(),
            http,
        })
    }

    /// Call a Tencent Cloud API action with a JSON body.
    /// Returns the `Response` object from the API.
    pub async fn call_api(
        &self,
        action: &str,
        version: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let body_str = serde_json::to_string(body).context("failed to serialize request body")?;
        let timestamp = chrono::Utc::now().timestamp();
        let date = DateTime::<Utc>::from_timestamp(timestamp, 0)
            .unwrap_or_else(Utc::now)
            .format("%Y-%m-%d")
            .to_string();

        // Stage 1: canonical request
        let hashed_payload = hex_encode(&sha256(body_str.as_bytes()));
        let canonical_headers = format!(
            "content-type:application/json; charset=utf-8\nhost:{}\nx-tc-action:{}\n",
            self.host,
            action.to_lowercase()
        );
        let signed_headers = "content-type;host;x-tc-action";
        let canonical_request = format!(
            "POST\n/\n\n{}\n{}\n{}",
            canonical_headers, signed_headers, hashed_payload
        );

        // Stage 2: string to sign
        let credential_scope = format!("{}/{}/tc3_request", date, self.service);
        let hashed_canonical_request = hex_encode(&sha256(canonical_request.as_bytes()));
        let string_to_sign = format!(
            "TC3-HMAC-SHA256\n{}\n{}\n{}",
            timestamp, credential_scope, hashed_canonical_request
        );

        // Stage 3: derive signing key
        let secret_date = hmac_sha256(
            format!("TC3{}", self.secret_key).as_bytes(),
            date.as_bytes(),
        );
        let secret_service = hmac_sha256(&secret_date, self.service.as_bytes());
        let secret_signing = hmac_sha256(&secret_service, b"tc3_request");

        // Stage 4: compute signature
        let signature = hex_encode(&hmac_sha256(&secret_signing, string_to_sign.as_bytes()));

        // Stage 5: authorization header
        let authorization = format!(
            "TC3-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.secret_id, credential_scope, signed_headers, signature
        );

        let url = format!("https://{}/", self.host);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", &authorization)
            .header("Content-Type", "application/json; charset=utf-8")
            .header("Host", &self.host)
            .header("X-TC-Action", action)
            .header("X-TC-Version", version)
            .header("X-TC-Region", &self.region)
            .header("X-TC-Timestamp", timestamp.to_string())
            .body(body_str)
            .send()
            .await
            .context("failed to send Tencent Cloud API request")?;

        let status = resp.status();
        let resp_text = resp.text().await.context("failed to read response body")?;

        if !status.is_success() {
            bail!(
                "Tencent Cloud API HTTP {} — {}\nresponse: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("error"),
                truncate(&resp_text, 500)
            );
        }

        let parsed: serde_json::Value = serde_json::from_str(&resp_text).with_context(|| {
            format!(
                "failed to parse Tencent Cloud API response: {}",
                truncate(&resp_text, 500)
            )
        })?;

        // Check for API-level error
        if let Some(error) = parsed.get("Response").and_then(|r| r.get("Error")) {
            let code = error
                .get("Code")
                .and_then(|c| c.as_str())
                .unwrap_or("unknown");
            let message = error
                .get("Message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            bail!("Tencent Cloud API error: {} — {}", code, message);
        }

        let response = parsed
            .get("Response")
            .context("response missing Response field")?
            .clone();

        Ok(response)
    }
}

fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key error");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{:02x}", b)).collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
