use alloy_primitives::{Address, B256, U256};
use eyre::eyre;
use jsonrpsee::core::Serialize;
use reqwest::{header, Client, Response, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info};
use url::Url;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrMessage {
    error: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub bid_deadline: f64,
    pub chain_id: u64,
    pub stake_contract: Address,
    pub tx_stream_cutoff: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Commitment {
    pub instant_unstake_data: Option<String>,
    pub latest_commitment_hash: B256,
    pub previous_commitment_hash: B256,
    pub stake_channel_nonce: U256,
    pub stake_commitment_nonce: U256,
    pub stake_notary_signature: Option<String>,
    pub stake_spent_amount: U256,
    pub staker_address: Address,
    pub staker_signature: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PaymentChannelState {
    pub latest_commitment: Commitment,
    pub staked_amount: U256,
    pub is_open: bool,
}

#[derive(Clone)]
pub struct ServoHttpClient {
    client: Client,
    pub staker_address: Address,
    base_url: Url,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BidResult {
    pub bid_id: String,
}

impl ServoHttpClient {
    pub fn new(bearer_token: String, base_url: Url, staker_address: Address) -> eyre::Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, format!("Bearer {bearer_token}").parse()?);
        headers.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"));
        Ok(Self {
            client: Client::builder().default_headers(headers).build()?,
            staker_address,
            base_url,
        })
    }

    pub async fn get_info(&self) -> eyre::Result<Info> {
        let response = self.client.get(self.base_url.join("info")?).send().await?;
        Self::handle_response::<Info>(response).await
    }

    pub async fn open_payment_channel(&self) -> eyre::Result<PaymentChannelState> {
        #[derive(Debug, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct PaymentChannelRequest { staker_address: Address }
        let response = self
            .client
            .post(self.base_url.join("paymentChannels")?)
            .json(&PaymentChannelRequest { staker_address: self.staker_address })
            .send()
            .await?;
        Self::handle_response::<PaymentChannelState>(response).await
    }

    pub async fn get_payment_channel_state(&self) -> eyre::Result<PaymentChannelState> {
        let path = format!("paymentChannels/{:#x}", self.staker_address);
        let response = self.client.get(self.base_url.join(&path)?).send().await?;
        Self::handle_response::<PaymentChannelState>(response).await
    }

    pub async fn bid(&self, bid: &Value) -> eyre::Result<BidResult> {
        let request_start = std::time::Instant::now();
        debug!("📡 Sending bid request to SERVO HTTP API");
        let response = self
            .client
            .post(self.base_url.join("bids")?)
            .json(bid)
            .send()
            .await?;
        let status = response.status();
        let response_time_ms = request_start.elapsed().as_millis();
        if status.is_success() {
            debug!(status = %status, response_time_ms = response_time_ms, "✅ SERVO HTTP response received successfully");
        } else {
            info!(status = %status, response_time_ms = response_time_ms, "❌ SERVO HTTP response failed");
        }
        Self::handle_response::<BidResult>(response).await
    }

    async fn handle_response<T>(response: Response) -> eyre::Result<T>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        let status = response.status();
        if status.is_success() {
            let parsed = response.json::<T>().await?;
            Ok(parsed)
        } else if status == StatusCode::BAD_REQUEST {
            let m = response.json::<ErrMessage>().await?;
            Err(eyre!("Status {}: {}", status, m.error))
        } else {
            let m = response.json::<Value>().await?;
            Err(eyre!("Status {}: {}", status, m))
        }
    }
}
