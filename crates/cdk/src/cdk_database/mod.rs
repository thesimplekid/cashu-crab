//! CDK Database

#[cfg(any(feature = "wallet", feature = "mint"))]
use std::collections::HashMap;
use std::fmt::Debug;

#[cfg(any(feature = "wallet", feature = "mint"))]
use async_trait::async_trait;
use thiserror::Error;

#[cfg(feature = "mint")]
use crate::mint::MintKeySetInfo;
#[cfg(feature = "wallet")]
use crate::nuts::State;
#[cfg(feature = "mint")]
use crate::nuts::{BlindSignature, MeltQuoteState, MintQuoteState, Proof};
#[cfg(any(feature = "wallet", feature = "mint"))]
use crate::nuts::{CurrencyUnit, Id, Proofs, PublicKey};
#[cfg(feature = "wallet")]
use crate::nuts::{KeySetInfo, Keys, MintInfo, SpendingConditions};
#[cfg(feature = "mint")]
use crate::secret::Secret;
#[cfg(feature = "wallet")]
use crate::types::ProofInfo;
#[cfg(any(feature = "wallet", feature = "mint"))]
use crate::types::{MeltQuote, MintQuote};
#[cfg(feature = "wallet")]
use crate::url::UncheckedUrl;

#[cfg(feature = "mint")]
pub mod mint_memory;
#[cfg(feature = "wallet")]
pub mod wallet_memory;

#[cfg(feature = "wallet")]
pub use wallet_memory::WalletMemoryDatabase;

/// CDK_database error
#[derive(Debug, Error)]
pub enum Error {
    /// Database Error
    #[error(transparent)]
    Database(Box<dyn std::error::Error + Send + Sync>),
    /// CDK Error
    #[error(transparent)]
    Cdk(#[from] crate::error::Error),
    /// NUT01 Error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut00::Error),
    /// Unknown Quote
    #[error("Unknown Quote")]
    UnknownQuote,
}

/// Wallet Database trait
#[cfg(feature = "wallet")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait WalletDatabase: Debug {
    /// Wallet Database Error
    type Err: Into<Error> + From<Error>;

    /// Add Mint to storage
    async fn add_mint(
        &self,
        mint_url: UncheckedUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), Self::Err>;
    /// Remove Mint from storage
    async fn remove_mint(&self, mint_url: UncheckedUrl) -> Result<(), Self::Err>;
    /// Get mint from storage
    async fn get_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Self::Err>;
    /// Get all mints from storage
    async fn get_mints(&self) -> Result<HashMap<UncheckedUrl, Option<MintInfo>>, Self::Err>;
    /// Update mint url
    async fn update_mint_url(
        &self,
        old_mint_url: UncheckedUrl,
        new_mint_url: UncheckedUrl,
    ) -> Result<(), Self::Err>;

    /// Add mint keyset to storage
    async fn add_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Self::Err>;
    /// Get mint keysets for mint url
    async fn get_mint_keysets(
        &self,
        mint_url: UncheckedUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, Self::Err>;
    /// Get mint keyset by id
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Self::Err>;

    /// Add mint quote to storage
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err>;
    /// Get mint quote from storage
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err>;
    /// Get mint quotes from storage
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err>;
    /// Remove mint quote from storage
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    /// Add melt quote to storage
    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err>;
    /// Get melt quote from storage
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err>;
    /// Remove melt quote from storage
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    /// Add [`Keys`] to storage
    async fn add_keys(&self, keys: Keys) -> Result<(), Self::Err>;
    /// Get [`Keys`] from storage
    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Self::Err>;
    /// Remove [`Keys`] from storage
    async fn remove_keys(&self, id: &Id) -> Result<(), Self::Err>;

    /// Add [`Proofs`] to storage
    async fn add_proofs(&self, proof_info: Vec<ProofInfo>) -> Result<(), Self::Err>;
    /// Get proofs from storage
    async fn get_proofs(
        &self,
        mint_url: Option<UncheckedUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Option<Vec<ProofInfo>>, Self::Err>;
    /// Remove proofs from storage
    async fn remove_proofs(&self, proofs: &Proofs) -> Result<(), Self::Err>;

    /// Set Proof state
    async fn set_proof_state(&self, y: PublicKey, state: State) -> Result<(), Self::Err>;

    /// Increment Keyset counter
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Self::Err>;
    /// Get current Keyset counter
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u32>, Self::Err>;

    /// Get when nostr key was last checked
    async fn get_nostr_last_checked(
        &self,
        verifying_key: &PublicKey,
    ) -> Result<Option<u32>, Self::Err>;
    /// Update last checked time
    async fn add_nostr_last_checked(
        &self,
        verifying_key: PublicKey,
        last_checked: u32,
    ) -> Result<(), Self::Err>;
}

/// Mint Database trait
#[cfg(feature = "mint")]
#[async_trait]
pub trait MintDatabase {
    /// Mint Database Error
    type Err: Into<Error> + From<Error>;

    /// Add Active Keyset
    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err>;
    /// Get Active Keyset
    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err>;
    /// Get all Active Keyset
    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err>;

    /// Add [`MintQuote`]
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err>;
    /// Get [`MintQuote`]
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err>;
    /// Update state of [`MintQuote`]
    async fn update_mint_quote_state(
        &self,
        quote_id: &str,
        state: MintQuoteState,
    ) -> Result<MintQuoteState, Self::Err>;
    /// Get all [`MintQuote`]s
    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err>;
    /// Get Mint Quotes
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err>;
    /// Remove [`MintQuote`]
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    /// Add [`MeltQuote`]
    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err>;
    /// Get [`MeltQuote`]
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err>;
    /// Update [`MeltQuote`] state
    async fn update_melt_quote_state(
        &self,
        quote_id: &str,
        state: MeltQuoteState,
    ) -> Result<MeltQuoteState, Self::Err>;
    /// Get all [`MeltQuote`]s
    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Self::Err>;
    /// Remove [`MeltQuote`]
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err>;

    /// Add [`MintKeySetInfo`]
    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err>;
    /// Get [`MintKeySetInfo`]
    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err>;
    /// Get [`MintKeySetInfo`]s
    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err>;

    /// Add spent [`Proofs`]
    async fn add_spent_proofs(&self, proof: Proofs) -> Result<(), Self::Err>;
    /// Get spent [`Proof`] by secret
    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Self::Err>;
    /// Get spent [`Proof`] by y
    async fn get_spent_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err>;

    /// Add pending [`Proofs`]
    async fn add_pending_proofs(&self, proof: Proofs) -> Result<(), Self::Err>;
    /// Get pending [`Proof`] by secret
    async fn get_pending_proof_by_secret(
        &self,
        secret: &Secret,
    ) -> Result<Option<Proof>, Self::Err>;
    /// Get pending [`Proof`] by y
    async fn get_pending_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err>;
    /// Remove pending [`Proofs`]
    async fn remove_pending_proofs(&self, secret: Vec<&Secret>) -> Result<(), Self::Err>;

    /// Add [`BlindSignature`]
    async fn add_blinded_signature(
        &self,
        blinded_message: PublicKey,
        blinded_signature: BlindSignature,
    ) -> Result<(), Self::Err>;
    /// Get [`BlindSignature`]
    async fn get_blinded_signature(
        &self,
        blinded_message: &PublicKey,
    ) -> Result<Option<BlindSignature>, Self::Err>;
    /// Get [`BlindSignature`]s
    async fn get_blinded_signatures(
        &self,
        blinded_messages: Vec<PublicKey>,
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err>;
}
