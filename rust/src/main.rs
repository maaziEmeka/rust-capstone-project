#![allow(unused)]
use bitcoin::hex::DisplayHex;
use bitcoincore_rpc::bitcoin::{Address, Amount, Network, SignedAmount};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use serde::Deserialize;
use serde_json::json;
use std::fs::File;
use std::io::Write;
use std::str::FromStr;

// Node access params
const RPC_URL: &str = "http://127.0.0.1:18443"; // Default regtest RPC port
const RPC_USER: &str = "alice";
const RPC_PASS: &str = "password";

// You can use calls not provided in RPC lib API using the generic `call` function.
// An example of using the `send` RPC call, which doesn't have exposed API.
// You can also use serde_json `Deserialize` derivation to capture the returned json result.
fn send(rpc: &Client, addr: &str) -> bitcoincore_rpc::Result<String> {
    let args = [
        json!([{addr : 100 }]), // recipient address
        json!(null),            // conf target
        json!(null),            // estimate mode
        json!(null),            // fee rate in sats/vb
        json!(null),            // Empty option object
    ];

    #[derive(Deserialize)]
    struct SendResult {
        complete: bool,
        txid: String,
    }
    let send_result = rpc.call::<SendResult>("send", &args)?;
    assert!(send_result.complete);
    Ok(send_result.txid)
}

fn main() -> bitcoincore_rpc::Result<()> {
    // Connect to Bitcoin Core RPC
    let rpc = Client::new(
        RPC_URL,
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )?;

    // Get blockchain info
    let blockchain_info = rpc.get_blockchain_info()?;
    println!("Blockchain Info: {:?}", blockchain_info);

    // Create/Load the wallets, named 'Miner' and 'Trader'. Have logic to optionally create/load them if they do not exist or not loaded already.

    let existing_wallets = rpc.list_wallet_dir()?;
    let loaded_wallets = rpc.list_wallets()?;
    let target_wallets = ["Miner", "Trader"];
    for wallet_name in target_wallets {
        if !existing_wallets.contains(&wallet_name.to_string()) {
            println!("Creating {} wallet", wallet_name);
            rpc.create_wallet(wallet_name, None, None, None, None)?;
        } else if !loaded_wallets.contains(&wallet_name.to_string()) {
            println!("Loading {} wallet", wallet_name);
            rpc.load_wallet(wallet_name)?;
        } else {
            println!("{} Wallet already loaded", wallet_name);
        }
    }

    // Generate spendable balances in the Miner wallet. How many blocks needs to be mined?

    let miner_rpc = Client::new(
        &format!("{}/wallet/Miner", RPC_URL),
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )?;
    let uncheckd_miner_address = miner_rpc.get_new_address(None, None)?;
    let miner_address = uncheckd_miner_address
        .require_network(Network::Regtest)
        .unwrap();
    println!("Generating 101 blocks to Miner address {}", miner_address);
    miner_rpc.generate_to_address(101, &miner_address)?;

    // Load Trader wallet and generate a new address

    let trader_rpc = Client::new(
        &format!("{}/wallet/Trader", RPC_URL),
        Auth::UserPass(RPC_USER.to_owned(), RPC_PASS.to_owned()),
    )?;
    let uncheckedtrader_address = trader_rpc.get_new_address(None, None)?;
    let trader_address = uncheckedtrader_address
        .require_network(Network::Regtest)
        .unwrap();
    println!("Sending 20 BTC to Trader address: {}", trader_address);

    // Send 20 BTC from Miner to Trader

    let send_tx = miner_rpc.send_to_address(
        &trader_address,
        Amount::from_btc(20.0)?,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;
    println!("Generated transaction txid {}", send_tx);

    // Check transaction in mempool

    let mempool_entry = miner_rpc.get_mempool_entry(&send_tx)?;
    println!("Mempool entry for txid {:?}", mempool_entry);

    // Mine 1 block to confirm the transaction

    miner_rpc.generate_to_address(1, &miner_address)?;
    let trader_balance = trader_rpc.get_balance(None, None)?;
    println!("Trader balance: {}", trader_balance);

    // Extract all required transaction details

    let tx_details = miner_rpc.get_transaction(&send_tx, None)?;
    let fee = tx_details.fee.unwrap_or(SignedAmount::from_sat(0)).abs();
    let block_height = tx_details.info.blockheight.unwrap_or(0);
    let block_hash = tx_details.info.blockhash.unwrap();
    println!(
        "Transaction fee: {}, block height: {} , block hash: {}",
        fee, block_height, block_hash
    );

    // Get raw transaction details

    let tx_raw: serde_json::Value = miner_rpc.call(
        "getrawtransaction",
        &[send_tx.to_string().into(), true.into()],
    )?;

    //parse vout entries for address and amount

    let parse_addr_amount = |vout: serde_json::Value| -> Option<(Option<Address>, Amount)> {
        let addr = Address::from_str(vout["scriptPubKey"]["address"].as_str()?)
            .unwrap()
            .require_network(Network::Regtest)
            .ok();
        let amount = Amount::from_btc(vout["value"].as_f64()?).ok()?;
        Some((addr, amount))
    };

    //process the first vin to get input address and amount.

    let (input_address, input_amount) = tx_raw["vin"]
        .as_array()
        .and_then(|vins| vins.get(0))
        .and_then(|vin| {
            let prev_txid = vin["txid"].as_str()?;
            println!("Fetching previous transaction: {}", prev_txid);
            miner_rpc
                .call("getrawtransaction", &[prev_txid.into(), true.into()])
                .ok()
                .and_then(|prev_tx: serde_json::Value| {
                    prev_tx["vout"]
                        .as_array()?
                        .get(vin["vout"].as_u64()? as usize)
                        .cloned()
                        .and_then(parse_addr_amount)
                })
        })
        .unwrap_or_else(|| {
            println!("Warning: Failed to parse input address/amount using default values");
            (None, Amount::from_sat(0))
        });

    //Process vout to get Trader and change outputs

    let (trader_output_amount, change_address, change_amount) = tx_raw["vout"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .cloned()
        .filter_map(parse_addr_amount)
        .fold(
            (Amount::from_sat(0), None, Amount::from_sat(0)),
            |(trader_amt, change_addr, change_amt), (addr, amt)| {
                if addr == Some(trader_address.clone()) {
                    (amt, change_addr, change_amt)
                } else {
                    (trader_amt, addr, amt)
                }
            },
        );
    println!(
        "Trader output amount: {}, Change address: {}, Change amount: {}",
        trader_output_amount.to_btc(),
        change_address
            .as_ref()
            .map_or(String::new(), |a| a.to_string()),
        change_amount.to_btc()
    );

    // Write the data to ../out.txt in the specified format given in readme.md

    let mut file = File::create("../out.txt")?;
    writeln!(file, "{}", send_tx)?;
    writeln!(
        file,
        "{}",
        input_address.map_or(String::new(), |a| a.to_string())
    )?;
    writeln!(file, "{}", input_amount.to_btc())?;
    writeln!(file, "{}", trader_address)?;
    writeln!(file, "{}", trader_output_amount.to_btc())?;
    writeln!(
        file,
        "{}",
        change_address.map_or(String::new(), |c| c.to_string())
    )?;
    writeln!(file, "{}", change_amount.to_btc())?;
    writeln!(file, "{}", fee.to_btc())?;
    writeln!(file, "{}", block_height)?;
    writeln!(file, "{}", block_hash)?;

    println!("Transaction Details written to out.txt");

    Ok(())
}
