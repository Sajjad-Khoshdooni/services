use anyhow::{ensure, Context, Result};
use ethcontract::{dyns::DynTransport, transaction::TransactionBuilder, Address, H256};
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Client, Url,
};
use serde::{Deserialize, Serialize};
use web3::types::{AccessList, Bytes};

#[async_trait::async_trait]
pub trait AccessListEstimating: Send + Sync {
    async fn estimate_access_list(
        &self,
        tx: &TransactionBuilder<DynTransport>,
    ) -> Result<AccessList> {
        self.estimate_access_lists(std::slice::from_ref(tx))
            .await
            .into_iter()
            .next()
            .unwrap()
    }
    async fn estimate_access_lists(
        &self,
        txs: &[TransactionBuilder<DynTransport>],
    ) -> Vec<Result<AccessList>>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TenderlyRequest {
    network_id: String,
    block_number: u64,
    from: Address,
    input: Bytes,
    to: Address,
    generate_access_list: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct TenderlyResponse {
    generated_access_list: Vec<AccessListItem>,
}

// Had to introduce copy of the web3 AccessList because tenderly responds with snake_case fields
// and tenderly storage_keys field does not exist if empty (it should be empty Vec instead)
#[derive(Debug, Clone, Deserialize)]
struct AccessListItem {
    /// Accessed address
    address: Address,
    /// Accessed storage keys
    #[serde(default)]
    storage_keys: Vec<H256>,
}

impl From<AccessListItem> for web3::types::AccessListItem {
    fn from(item: AccessListItem) -> Self {
        Self {
            address: item.address,
            storage_keys: item.storage_keys,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct BlockNumber {
    block_number: u64,
}

#[derive(Debug)]
pub struct TenderlyApi {
    url: Url,
    client: Client,
    header: HeaderMap,
    network_id: String,
}

impl TenderlyApi {
    #[allow(dead_code)]
    pub fn new(url: Url, api_key: &str, network_id: String) -> Self {
        Self {
            url,
            client: Client::new(),
            header: {
                let mut header = HeaderMap::new();
                header.insert("x-access-key", HeaderValue::from_str(api_key).unwrap());
                header
            },
            network_id,
        }
    }

    #[allow(dead_code)]
    async fn access_list(&self, body: TenderlyRequest) -> reqwest::Result<TenderlyResponse> {
        self.client
            .post(self.url.clone())
            .headers(self.header.clone())
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }

    async fn block_number(&self, network_id: String) -> reqwest::Result<BlockNumber> {
        self.client
            .get(format!(
                "https://api.tenderly.co/api/v1/network/{}/block-number",
                network_id
            ))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }
}

#[async_trait::async_trait]
impl AccessListEstimating for TenderlyApi {
    async fn estimate_access_lists(
        &self,
        txs: &[TransactionBuilder<DynTransport>],
    ) -> Vec<Result<AccessList>> {
        futures::future::join_all(txs.iter().map(|tx| async {
            let input = tx.data.clone().context("transaction data does not exist")?;
            let from = tx
                .from
                .clone()
                .context("transaction from does not exist")?
                .address();
            let to = tx.to.context("transaction to does not exist")?;
            let block_number = self.block_number(self.network_id.clone()).await?;

            let tenderly_request = TenderlyRequest {
                network_id: self.network_id.clone(),
                block_number: block_number.block_number,
                from,
                input,
                to,
                generate_access_list: true,
            };

            let response = self.access_list(tenderly_request).await?;
            ensure!(
                !response.generated_access_list.is_empty(),
                "empty access list"
            );
            Ok(response
                .generated_access_list
                .into_iter()
                .map(Into::into)
                .collect())
        }))
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethcontract::{Account, H160};
    use hex_literal::hex;
    use serde_json::json;
    use shared::{transport::create_env_test_transport, Web3};

    #[tokio::test]
    #[ignore]
    async fn real_request_access_list() {
        let tenderly_api = TenderlyApi::new(
            // http://api.tenderly.co/api/v1/account/<USER_NAME>/project/<PROJECT_NAME>/simulate
            Url::parse(&std::env::var("TENDERLY_URL").unwrap()).unwrap(),
            &std::env::var("TENDERLY_API_KEY").unwrap(),
            "1".to_string(),
        );
        let request = TenderlyRequest {
            network_id: "1".to_string(),
            block_number: 14122310,
            from: H160::from_slice(&hex!("e92f359e6f05564849afa933ce8f62b8007a1d5d")),
            input: hex!("13d79a0b00000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000018000000000000000000000000000000000000000000000000000000000000005a000000000000000000000000000000000000000000000000000000000000000030000000000000000000000004e3fbd56cd56c3e72c1403e103b45db9da5b9d2b000000000000000000000000990f341946a3fdb507ae7e52d17851b87168017c000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000000000000000000000000000000000000000000300000000000000000000000000000000000000000000000000000006765a71600000000000000000000000000000000000000000000000000000007347b2e76f0000000000000000000000000000000000000000000000368237ac6c6ad709fe0000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000002200000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000000000000000000000000000098e073b579fd483eac8f10d5bd0b32c8c3bbd7e000000000000000000000000000000000000000000000000000000006765a71600000000000000000000000000000000000000000000000363ccb23497d69b5e10000000000000000000000000000000000000000000000000000000061f99a9c487b02c558d729abaf3ecf17881a4181e5bc2446429a0995142297e897b6eb37000000000000000000000000000000000000000000000000000000000e93a6a0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006765a716000000000000000000000000000000000000000000000000000000000000001600000000000000000000000000000000000000000000000000000000000000041c5a207f8688e853bdd7402727104da7b4094672dc8672c60840e5d0457e3be85295c881e39e59070ea3b42a79de3c4d6ba7a41d10e1883b2aafc6c77be0518ea1c00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000020000000000000000000000001aefff55c6b6a53f6b63eab65025446024ebc8e3000000000000000000000000000000000000000000000000de9babded1fb850e00000000000000000000000000000000000000000000000000000001d4734cf00000000000000000000000000000000000000000000000000000000061f99f38487b02c558d729abaf3ecf17881a4181e5bc2446429a0995142297e897b6eb3700000000000000000000000000000000000000000000000001e9db2b61bfd6500000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000de9babded1fb850e0000000000000000000000000000000000000000000000000000000000000160000000000000000000000000000000000000000000000000000000000000004125fa0bacb9c8806fe80910b005e10d9aa5dbb02bd0a66ccdc549d92304625fd95f6e07b36480389e6067894c2bc4ad45617aa11449d5a01b4dcf0a3bf34a33911b00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000cc00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000a40000000000000000000000000def1c0ded9bec7f1a1670819833240f027b25eff000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000968415565b0000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb480000000000000000000000004e3fbd56cd56c3e72c1403e103b45db9da5b9d2b00000000000000000000000000000000000000000000000000000006765a7160000000000000000000000000000000000000000000000036585ad5a25d351d2a00000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000003000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000003c0000000000000000000000000000000000000000000000000000000000000070000000000000000000000000000000000000000000000000000000000000000150000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2000000000000000000000000000000000000000000000000000000000000012000000000000000000000000000000000000000000000000000000000000002c000000000000000000000000000000000000000000000000000000000000002c000000000000000000000000000000000000000000000000000000000000002a000000000000000000000000000000000000000000000000000000006765a716000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000012556e697377617056330000000000000000000000000000000000000000000000000000000000000000000006765a71600000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000e592427a0aece92de3edee1f18e0157c058615640000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000002ba0b86991c6218b36c1d19d4a2e9eb0ce3606eb480001f4c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000015000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000002e000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc20000000000000000000000004e3fbd56cd56c3e72c1403e103b45db9da5b9d2b000000000000000000000000000000000000000000000000000000000000012000000000000000000000000000000000000000000000000000000000000002a000000000000000000000000000000000000000000000000000000000000002a00000000000000000000000000000000000000000000000000000000000000280ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000143757276650000000000000000000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff000000000000000000000000000000000000000000000036585ad5a25d351d2900000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000080000000000000000000000000b576491f1e6e5e62f1d8f26062ee822b40b0e0d465b2489b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000007000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000000000000000000000000000003000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2000000000000000000000000eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0000000000000000000000000000000000000000000000000000000000000000869584cd0000000000000000000000009008d19f58aabd9ed0d60971565aa8510560ab410000000000000000000000000000000000000000000000649e79ae6861f99856000000000000000000000000000000000000000000000000000000000000000000000000def1c0ded9bec7f1a1670819833240f027b25eff0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000001486af479b20000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000de9babded1fb850e00000000000000000000000000000000000000000000000000000001d561592a00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000042990f341946a3fdb507ae7e52d17851b87168017c000bb8c02aaa39b223fe8d0a0e5c4f27ead9083c756cc20001f4a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000000000000000000000000000000000000000869584cd0000000000000000000000009008d19f58aabd9ed0d60971565aa8510560ab410000000000000000000000000000000000000000000000a5b49e4eb461f998560000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
            to: H160::from_slice(&hex!("9008d19f58aabd9ed0d60971565aa8510560ab41")),
            generate_access_list: true,
        };
        let access_list = tenderly_api.access_list(request).await.unwrap();
        dbg!(access_list);
    }

    #[tokio::test]
    #[ignore]
    async fn real_request_estimate() {
        let tenderly_api = TenderlyApi::new(
            // http://api.tenderly.co/api/v1/account/<USER_NAME>/project/<PROJECT_NAME>/simulate
            Url::parse(&std::env::var("TENDERLY_URL").unwrap()).unwrap(),
            &std::env::var("TENDERLY_API_KEY").unwrap(),
            "1".to_string(),
        );
        let http = create_env_test_transport();
        let web3 = Web3::new(http);
        let account = Account::Local(
            H160::from_slice(&hex!("e92f359e6f05564849afa933ce8f62b8007a1d5d")),
            None,
        );
        let data: Bytes = hex!("13d79a0b00000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000018000000000000000000000000000000000000000000000000000000000000005a000000000000000000000000000000000000000000000000000000000000000030000000000000000000000004e3fbd56cd56c3e72c1403e103b45db9da5b9d2b000000000000000000000000990f341946a3fdb507ae7e52d17851b87168017c000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000000000000000000000000000000000000000000300000000000000000000000000000000000000000000000000000006765a71600000000000000000000000000000000000000000000000000000007347b2e76f0000000000000000000000000000000000000000000000368237ac6c6ad709fe0000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000002200000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000000000000000000000000000098e073b579fd483eac8f10d5bd0b32c8c3bbd7e000000000000000000000000000000000000000000000000000000006765a71600000000000000000000000000000000000000000000000363ccb23497d69b5e10000000000000000000000000000000000000000000000000000000061f99a9c487b02c558d729abaf3ecf17881a4181e5bc2446429a0995142297e897b6eb37000000000000000000000000000000000000000000000000000000000e93a6a0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006765a716000000000000000000000000000000000000000000000000000000000000001600000000000000000000000000000000000000000000000000000000000000041c5a207f8688e853bdd7402727104da7b4094672dc8672c60840e5d0457e3be85295c881e39e59070ea3b42a79de3c4d6ba7a41d10e1883b2aafc6c77be0518ea1c00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000020000000000000000000000001aefff55c6b6a53f6b63eab65025446024ebc8e3000000000000000000000000000000000000000000000000de9babded1fb850e00000000000000000000000000000000000000000000000000000001d4734cf00000000000000000000000000000000000000000000000000000000061f99f38487b02c558d729abaf3ecf17881a4181e5bc2446429a0995142297e897b6eb3700000000000000000000000000000000000000000000000001e9db2b61bfd6500000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000de9babded1fb850e0000000000000000000000000000000000000000000000000000000000000160000000000000000000000000000000000000000000000000000000000000004125fa0bacb9c8806fe80910b005e10d9aa5dbb02bd0a66ccdc549d92304625fd95f6e07b36480389e6067894c2bc4ad45617aa11449d5a01b4dcf0a3bf34a33911b00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000cc00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000a40000000000000000000000000def1c0ded9bec7f1a1670819833240f027b25eff000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000968415565b0000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb480000000000000000000000004e3fbd56cd56c3e72c1403e103b45db9da5b9d2b00000000000000000000000000000000000000000000000000000006765a7160000000000000000000000000000000000000000000000036585ad5a25d351d2a00000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000003000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000003c0000000000000000000000000000000000000000000000000000000000000070000000000000000000000000000000000000000000000000000000000000000150000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2000000000000000000000000000000000000000000000000000000000000012000000000000000000000000000000000000000000000000000000000000002c000000000000000000000000000000000000000000000000000000000000002c000000000000000000000000000000000000000000000000000000000000002a000000000000000000000000000000000000000000000000000000006765a716000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000012556e697377617056330000000000000000000000000000000000000000000000000000000000000000000006765a71600000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000e592427a0aece92de3edee1f18e0157c058615640000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000002ba0b86991c6218b36c1d19d4a2e9eb0ce3606eb480001f4c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000015000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000002e000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc20000000000000000000000004e3fbd56cd56c3e72c1403e103b45db9da5b9d2b000000000000000000000000000000000000000000000000000000000000012000000000000000000000000000000000000000000000000000000000000002a000000000000000000000000000000000000000000000000000000000000002a00000000000000000000000000000000000000000000000000000000000000280ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000143757276650000000000000000000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff000000000000000000000000000000000000000000000036585ad5a25d351d2900000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000080000000000000000000000000b576491f1e6e5e62f1d8f26062ee822b40b0e0d465b2489b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000007000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000000000000000000000000000003000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2000000000000000000000000eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0000000000000000000000000000000000000000000000000000000000000000869584cd0000000000000000000000009008d19f58aabd9ed0d60971565aa8510560ab410000000000000000000000000000000000000000000000649e79ae6861f99856000000000000000000000000000000000000000000000000000000000000000000000000def1c0ded9bec7f1a1670819833240f027b25eff0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000001486af479b20000000000000000000000000000000000000000000000000000000000000080000000000000000000000000000000000000000000000000de9babded1fb850e00000000000000000000000000000000000000000000000000000001d561592a00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000042990f341946a3fdb507ae7e52d17851b87168017c000bb8c02aaa39b223fe8d0a0e5c4f27ead9083c756cc20001f4a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000000000000000000000000000000000000000869584cd0000000000000000000000009008d19f58aabd9ed0d60971565aa8510560ab410000000000000000000000000000000000000000000000a5b49e4eb461f998560000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into();

        let tx = TransactionBuilder::new(web3.clone())
            .from(account)
            .to(H160::from_slice(&hex!(
                "9008d19f58aabd9ed0d60971565aa8510560ab41"
            )))
            .data(data);

        let access_list = tenderly_api.estimate_access_lists(&[tx]).await;
        dbg!(access_list);
    }

    #[test]
    fn serialize_deserialize_request() {
        let request = TenderlyRequest {
            network_id: "1".to_string(),
            block_number: 14122310,
            from: H160::from_slice(&hex!("e92f359e6f05564849afa933ce8f62b8007a1d5d")),
            input: hex!("13d79a0b00000000000000000000000000000000000000000000").into(),
            to: H160::from_slice(&hex!("9008d19f58aabd9ed0d60971565aa8510560ab41")),
            generate_access_list: true,
        };

        let json = json!({
            "network_id": "1",
            "block_number": 14122310,
            "from": "0xe92f359e6f05564849afa933ce8f62b8007a1d5d",
            "input": "0x13d79a0b00000000000000000000000000000000000000000000",
            "to": "0x9008d19f58aabd9ed0d60971565aa8510560ab41",
            "generate_access_list": true
        });

        assert_eq!(serde_json::to_value(&request).unwrap(), json);
        assert_eq!(
            serde_json::from_value::<TenderlyRequest>(json).unwrap(),
            request
        );
    }
}