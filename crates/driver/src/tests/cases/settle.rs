use {
    super::SOLVER_NAME,
    crate::{
        domain::competition,
        infra::{self, config::cli},
        tests::{self, hex_address, setup},
    },
    itertools::Itertools,
    serde_json::json,
};

/// Test that the /settle endpoint behaves as expected.
#[ignore]
#[tokio::test]
async fn test() {
    // Set up the uniswap swap.
    let setup::blockchain::Uniswap {
        web3,
        settlement,
        token_a,
        token_b,
        admin,
        domain_separator,
        user_fee,
        token_a_in_amount,
        token_b_out_amount,
        weth,
        admin_secret_key,
        interactions,
        solver_address,
    } = setup::blockchain::uniswap::setup().await;

    // Values for the auction.
    let sell_token = token_a.address();
    let buy_token = token_b.address();
    let sell_amount = token_a_in_amount;
    let buy_amount = token_b_out_amount;
    let valid_to = u32::MAX;
    let boundary = tests::boundary::Order {
        sell_token,
        buy_token,
        sell_amount,
        buy_amount,
        valid_to,
        user_fee,
        side: competition::order::Side::Sell,
        secret_key: admin_secret_key,
        domain_separator,
        owner: admin,
    };
    let gas_price = web3.eth().gas_price().await.unwrap().to_string();
    let now = infra::time::Now::Fake(chrono::Utc::now());
    let deadline = now.now() + chrono::Duration::days(30);
    let interactions = interactions
        .into_iter()
        .map(|(address, interaction)| {
            json!({
                "kind": "custom",
                "internalize": false,
                "target": hex_address(address),
                "value": "0",
                "callData": format!("0x{}", hex::encode(interaction)),
                "allowances": [],
                "inputs": [],
                "outputs": [],
            })
        })
        .collect_vec();

    // Set up the solver.
    let solver = setup::solver::setup(setup::solver::Config {
        name: SOLVER_NAME.to_owned(),
        absolute_slippage: "0".to_owned(),
        relative_slippage: "0.0".to_owned(),
        address: hex_address(solver_address),
        solve: vec![setup::solver::Solve {
            req: json!({
                "id": "1",
                "tokens": {
                    hex_address(sell_token): {
                        "decimals": null,
                        "symbol": null,
                        "referencePrice": buy_amount.to_string(),
                        "availableBalance": "0",
                        "trusted": false,
                    },
                    hex_address(buy_token): {
                        "decimals": null,
                        "symbol": null,
                        "referencePrice": sell_amount.to_string(),
                        "availableBalance": "0",
                        "trusted": false,
                    }
                },
                "orders": [
                    {
                        "uid": boundary.uid(),
                        "sellToken": hex_address(sell_token),
                        "buyToken": hex_address(buy_token),
                        "sellAmount": sell_amount.to_string(),
                        "buyAmount": buy_amount.to_string(),
                        "feeAmount": "0",
                        "kind": "sell",
                        "partiallyFillable": false,
                        "class": "market",
                        "reward": 0.1,
                    }
                ],
                "liquidity": [],
                "effectiveGasPrice": gas_price,
                "deadline": deadline - competition::SolverTimeout::solving_time_buffer(),
            }),
            res: json!({
                "prices": {
                    hex_address(sell_token): buy_amount.to_string(),
                    hex_address(buy_token): sell_amount.to_string(),
                },
                "trades": [
                    {
                        "kind": "fulfillment",
                        "order": boundary.uid(),
                        "executedAmount": sell_amount.to_string(),
                    }
                ],
                "interactions": interactions
            }),
        }],
    })
    .await;

    // Set up the driver.
    let client = setup::driver::setup(setup::driver::Config {
        now,
        contracts: cli::ContractAddresses {
            gp_v2_settlement: Some(settlement.address()),
            weth: Some(weth.address()),
        },
        file: setup::driver::ConfigFile::Create(vec![solver]),
    })
    .await;

    // Call /solve.
    let solution = client
        .solve(
            SOLVER_NAME,
            json!({
                "id": "1",
                "tokens": {
                    hex_address(sell_token): {
                        "availableBalance": "0",
                        "trusted": false,
                        "referencePrice": buy_amount.to_string(),
                    },
                    hex_address(buy_token): {
                        "availableBalance": "0",
                        "trusted": false,
                        "referencePrice": sell_amount.to_string(),
                    }
                },
                "orders": [
                    {
                        "uid": boundary.uid(),
                        "sellToken": hex_address(sell_token),
                        "buyToken": hex_address(buy_token),
                        "sellAmount": sell_amount.to_string(),
                        "buyAmount": buy_amount.to_string(),
                        "solverFee": "0",
                        "userFee": user_fee.to_string(),
                        "validTo": valid_to,
                        "kind": "sell",
                        "owner": hex_address(admin),
                        "partiallyFillable": false,
                        "executed": "0",
                        "interactions": [],
                        "class": "market",
                        "appData": "0x0000000000000000000000000000000000000000000000000000000000000000",
                        "reward": 0.1,
                        "signingScheme": "eip712",
                        "signature": format!("0x{}", hex::encode(boundary.signature()))
                    }
                ],
                "effectiveGasPrice": gas_price,
                "deadline": deadline,
            }),
        )
        .await;

    let solution_id = solution.get("id").unwrap().as_str().unwrap();
    let old_tx_count = web3
        .eth()
        .transaction_count(solver_address, None)
        .await
        .unwrap();
    let old_balance = web3.eth().balance(solver_address, None).await.unwrap();
    let old_token_a = token_a.balance_of(admin).call().await.unwrap();
    let old_token_b = token_b.balance_of(admin).call().await.unwrap();

    client.settle(SOLVER_NAME, solution_id).await;

    // Assert.
    let new_tx_count = web3
        .eth()
        .transaction_count(solver_address, None)
        .await
        .unwrap();
    let new_balance = web3.eth().balance(solver_address, None).await.unwrap();
    let new_token_a = token_a.balance_of(admin).call().await.unwrap();
    let new_token_b = token_b.balance_of(admin).call().await.unwrap();
    assert_eq!(new_tx_count, old_tx_count + 1);
    // ETH balance is lower due to transaction fees.
    assert!(new_balance < old_balance);
    // The balance of the trader changes according to the swap.
    assert_eq!(new_token_a, old_token_a - token_a_in_amount - user_fee);
    assert_eq!(new_token_b, old_token_b + token_b_out_amount);
}