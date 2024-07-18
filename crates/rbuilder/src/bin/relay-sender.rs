use rbuilder::mev_boost::{rpc::TestDataGenerator, RelayClient, SubmitBlockRequest};
use std::str::FromStr;
use url::Url;

#[tokio::main]
async fn main() {
    let relay_url = Url::from_str("http://localhost:8080/").unwrap();
    let mut generator = TestDataGenerator::default();
    let relay = RelayClient::from_url(relay_url.clone(), None, None, None);
    let sub_relay = SubmitBlockRequest::Deneb(generator.create_deneb_submit_block_request());
    relay
        .submit_block(&sub_relay, true, true)
        .await
        .expect("OPS!");
    return;
}
