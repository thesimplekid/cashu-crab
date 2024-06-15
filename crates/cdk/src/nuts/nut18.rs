//! NUT-18: Melting Tokens Onchain
//!
//! <https://github.com/cashubtc/nuts/blob/main/18.md>

use serde::{Deserialize, Serialize};

use super::nut05::MeltMethodSettings;
use super::CurrencyUnit;
use crate::nuts::Proofs;
use crate::{Amount, Bolt11Invoice};

/// Melt quote request [NUT-18]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBtcOnchainRequest {
    /// Amount to be paid
    pub amount: Amount,
    /// Bitcoin onchain address to be paid
    pub address: Bolt11Invoice,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
}

/// Melt quote response [NUT-18]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBtcOnchainResponse {
    /// Quote Id
    pub quote: String,
    /// Description
    pub description: String,
    /// The amount that needs to be provided
    pub amount: u64,
    /// The fee that is required
    pub fee: u64,
    /// Whether the the request has been paid
    pub paid: bool,
    /// Unix timestamp until the quote is valid
    pub expiry: u64,
}

/// Melt BTC on chain Request [NUT-18]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBtcOnchianRequest {
    /// Quote ID
    pub quote: String,
    /// Proofs
    pub inputs: Proofs,
}

impl MeltBtcOnchianRequest {
    pub fn proofs_amount(&self) -> Amount {
        self.inputs.iter().map(|proof| proof.amount).sum()
    }
}

/// Melt Response [NUT-18]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBtcOnChainResponse {
    /// Indicate if payment was successful
    pub paid: bool,
    // TXID
    pub txid: Option<String>,
}

/// Melt Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    methods: Vec<MeltMethodSettings>,
}
