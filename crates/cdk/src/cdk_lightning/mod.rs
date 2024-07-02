//! CDK Mint Lightning

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use lightning_invoice::{Bolt11Invoice, ParseOrSemanticError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::nuts::{MeltQuoteState, MintQuoteState};

/// CDK Lightning Error
#[derive(Debug, Error)]
pub enum Error {
    /// Lightning Error
    #[error(transparent)]
    Lightning(Box<dyn std::error::Error + Send + Sync>),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// AnyHow Error
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
    /// Parse Error
    #[error(transparent)]
    Parse(#[from] ParseOrSemanticError),
}

/// MintLighting Trait
#[async_trait]
pub trait MintLightning {
    /// Mint Lightning Error
    type Err: Into<Error> + From<Error>;

    /// Create a new invoice
    async fn create_invoice(
        &self,
        msats: u64,
        description: String,
        unix_expiry: u64,
    ) -> Result<Bolt11Invoice, Self::Err>;

    /// Pay bolt11 invoice
    async fn pay_invoice(
        &self,
        bolt11: Bolt11Invoice,
        partial_msats: Option<u64>,
        max_fee_msats: Option<u64>,
    ) -> Result<PayInvoiceResponse, Self::Err>;

    /// Listen for invoices to be paid to the mint
    async fn wait_any_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Bolt11Invoice> + Send>>, Self::Err>;

    /// Check the status of an incoming payment
    async fn check_invoice_status(&self, payment_hash: &str) -> Result<MintQuoteState, Self::Err>;
}

/// Pay invoice response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayInvoiceResponse {
    /// Payment hash
    pub payment_hash: String,
    /// Payment Preimage
    pub payment_preimage: Option<String>,
    /// Status
    pub status: MeltQuoteState,
    /// Totoal Amount Spent in msats
    pub total_spent_msats: u64,
}
