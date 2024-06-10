//! Mint in memory database

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{Error, MintDatabase};
use crate::dhke::hash_to_curve;
use crate::mint::MintKeySetInfo;
use crate::nuts::{
    BlindSignature, CurrencyUnit, Id, MeltQuoteState, MintQuoteState, Proof, Proofs, PublicKey,
};
use crate::secret::Secret;
use crate::types::{MeltQuote, MintQuote};

/// Mint Memory Database
#[derive(Debug, Clone)]
pub struct MintMemoryDatabase {
    active_keysets: Arc<RwLock<HashMap<CurrencyUnit, Id>>>,
    keysets: Arc<RwLock<HashMap<Id, MintKeySetInfo>>>,
    mint_quotes: Arc<RwLock<HashMap<String, MintQuote>>>,
    melt_quotes: Arc<RwLock<HashMap<String, MeltQuote>>>,
    pending_proofs: Arc<RwLock<HashMap<[u8; 33], Proof>>>,
    spent_proofs: Arc<RwLock<HashMap<[u8; 33], Proof>>>,
    blinded_signatures: Arc<RwLock<HashMap<[u8; 33], BlindSignature>>>,
}

impl MintMemoryDatabase {
    /// Create new [`MintMemoryDatabase`]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        active_keysets: HashMap<CurrencyUnit, Id>,
        keysets: Vec<MintKeySetInfo>,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        pending_proofs: Proofs,
        spent_proofs: Proofs,
        blinded_signatures: HashMap<[u8; 33], BlindSignature>,
    ) -> Result<Self, Error> {
        Ok(Self {
            active_keysets: Arc::new(RwLock::new(active_keysets)),
            keysets: Arc::new(RwLock::new(
                keysets.into_iter().map(|k| (k.id, k)).collect(),
            )),
            mint_quotes: Arc::new(RwLock::new(
                mint_quotes.into_iter().map(|q| (q.id.clone(), q)).collect(),
            )),
            melt_quotes: Arc::new(RwLock::new(
                melt_quotes.into_iter().map(|q| (q.id.clone(), q)).collect(),
            )),
            pending_proofs: Arc::new(RwLock::new(
                pending_proofs
                    .into_iter()
                    .map(|p| (hash_to_curve(&p.secret.to_bytes()).unwrap().to_bytes(), p))
                    .collect(),
            )),
            spent_proofs: Arc::new(RwLock::new(
                spent_proofs
                    .into_iter()
                    .map(|p| (hash_to_curve(&p.secret.to_bytes()).unwrap().to_bytes(), p))
                    .collect(),
            )),
            blinded_signatures: Arc::new(RwLock::new(blinded_signatures)),
        })
    }
}

#[async_trait]
impl MintDatabase for MintMemoryDatabase {
    type Err = Error;

    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err> {
        self.active_keysets.write().await.insert(unit, id);
        Ok(())
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        Ok(self.active_keysets.read().await.get(unit).cloned())
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        Ok(self.active_keysets.read().await.clone())
    }

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err> {
        self.keysets.write().await.insert(keyset.id, keyset);
        Ok(())
    }

    async fn get_keyset_info(&self, keyset_id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
        Ok(self.keysets.read().await.get(keyset_id).cloned())
    }

    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
        Ok(self.keysets.read().await.values().cloned().collect())
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        self.mint_quotes
            .write()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        Ok(self.mint_quotes.read().await.get(quote_id).cloned())
    }

    async fn update_mint_quote_state(
        &self,
        quote_id: &str,
        state: MintQuoteState,
    ) -> Result<MintQuoteState, Self::Err> {
        let mut mint_quotes = self.mint_quotes.write().await;

        let mut quote = mint_quotes
            .get(quote_id)
            .cloned()
            .ok_or(Error::UnknownQuote)?;

        let current_state = quote.state;

        quote.state = state;

        mint_quotes.insert(quote_id.to_string(), quote.clone());

        Ok(current_state)
    }
    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let quotes = self.get_mint_quotes().await?;

        let quote = quotes
            .into_iter()
            .filter(|q| q.request.eq(request))
            .collect::<Vec<MintQuote>>()
            .first()
            .cloned();

        Ok(quote)
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        Ok(self.mint_quotes.read().await.values().cloned().collect())
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        self.mint_quotes.write().await.remove(quote_id);

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        self.melt_quotes
            .write()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
        Ok(self.melt_quotes.read().await.get(quote_id).cloned())
    }

    async fn update_melt_quote_state(
        &self,
        quote_id: &str,
        state: MeltQuoteState,
    ) -> Result<MeltQuoteState, Self::Err> {
        let mut melt_quotes = self.melt_quotes.write().await;

        let mut quote = melt_quotes
            .get(quote_id)
            .cloned()
            .ok_or(Error::UnknownQuote)?;

        let current_state = quote.state;

        quote.state = state;

        melt_quotes.insert(quote_id.to_string(), quote.clone());

        Ok(current_state)
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Self::Err> {
        Ok(self.melt_quotes.read().await.values().cloned().collect())
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        self.melt_quotes.write().await.remove(quote_id);

        Ok(())
    }

    async fn add_spent_proofs(&self, spent_proofs: Proofs) -> Result<(), Self::Err> {
        let mut proofs = self.spent_proofs.write().await;

        for proof in spent_proofs {
            let secret_point = hash_to_curve(&proof.secret.to_bytes())?;
            proofs.insert(secret_point.to_bytes(), proof);
        }
        Ok(())
    }

    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Self::Err> {
        Ok(self
            .spent_proofs
            .read()
            .await
            .get(&hash_to_curve(&secret.to_bytes())?.to_bytes())
            .cloned())
    }

    async fn get_spent_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        Ok(self.spent_proofs.read().await.get(&y.to_bytes()).cloned())
    }

    async fn add_pending_proofs(&self, pending_proofs: Proofs) -> Result<(), Self::Err> {
        let mut proofs = self.pending_proofs.write().await;

        for proof in pending_proofs {
            proofs.insert(hash_to_curve(&proof.secret.to_bytes())?.to_bytes(), proof);
        }
        Ok(())
    }

    async fn get_pending_proof_by_secret(
        &self,
        secret: &Secret,
    ) -> Result<Option<Proof>, Self::Err> {
        let secret_point = hash_to_curve(&secret.to_bytes())?;
        Ok(self
            .pending_proofs
            .read()
            .await
            .get(&secret_point.to_bytes())
            .cloned())
    }

    async fn get_pending_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        Ok(self.pending_proofs.read().await.get(&y.to_bytes()).cloned())
    }

    async fn remove_pending_proofs(&self, secrets: Vec<&Secret>) -> Result<(), Self::Err> {
        let mut proofs = self.pending_proofs.write().await;

        for secret in secrets {
            let secret_point = hash_to_curve(&secret.to_bytes())?;
            proofs.remove(&secret_point.to_bytes());
        }

        Ok(())
    }

    async fn add_blinded_signature(
        &self,
        blinded_message: PublicKey,
        blinded_signature: BlindSignature,
    ) -> Result<(), Self::Err> {
        self.blinded_signatures
            .write()
            .await
            .insert(blinded_message.to_bytes(), blinded_signature);
        Ok(())
    }

    async fn get_blinded_signature(
        &self,
        blinded_message: &PublicKey,
    ) -> Result<Option<BlindSignature>, Self::Err> {
        Ok(self
            .blinded_signatures
            .read()
            .await
            .get(&blinded_message.to_bytes())
            .cloned())
    }

    async fn get_blinded_signatures(
        &self,
        blinded_messages: Vec<PublicKey>,
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let mut signatures = Vec::with_capacity(blinded_messages.len());

        let blinded_signatures = self.blinded_signatures.read().await;

        for blinded_message in blinded_messages {
            let signature = blinded_signatures.get(&blinded_message.to_bytes()).cloned();

            signatures.push(signature)
        }

        Ok(signatures)
    }
}
