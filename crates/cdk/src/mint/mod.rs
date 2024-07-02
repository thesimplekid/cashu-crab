//! Cashu Mint

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, ExtendedPrivKey};
use bitcoin::secp256k1::{self, Secp256k1};
use error::Error;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use self::nut05::QuoteState;
use crate::cdk_database::{self, MintDatabase};
use crate::dhke::{hash_to_curve, sign_message, verify_message};
use crate::nuts::nut11::enforce_sig_flag;
use crate::nuts::*;
use crate::types::{MeltQuote, MintQuote};
use crate::url::UncheckedUrl;
use crate::util::unix_time;
use crate::Amount;

pub mod error;

/// Cashu Mint
#[derive(Clone)]
pub struct Mint {
    /// Mint Url
    pub mint_url: UncheckedUrl,
    mint_info: MintInfo,
    keysets: Arc<RwLock<HashMap<Id, MintKeySet>>>,
    secp_ctx: Secp256k1<secp256k1::All>,
    xpriv: ExtendedPrivKey,
    /// Mint Expected [`FeeReserve`]
    pub fee_reserve: FeeReserve,
    /// Mint Storage backend
    pub localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
}

impl Mint {
    /// Create new [`Mint`]
    pub async fn new(
        mint_url: &str,
        seed: &[u8],
        mint_info: MintInfo,
        localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
        min_fee_reserve: Amount,
        percent_fee_reserve: f32,
    ) -> Result<Self, Error> {
        let secp_ctx = Secp256k1::new();
        let xpriv =
            ExtendedPrivKey::new_master(bitcoin::Network::Bitcoin, seed).expect("RNG busted");

        let mut keysets = HashMap::new();
        let keysets_infos = localstore.get_keyset_infos().await?;

        match keysets_infos.is_empty() {
            false => {
                for keyset_info in keysets_infos {
                    if keyset_info.active {
                        let id = keyset_info.id;
                        let keyset = MintKeySet::generate_from_xpriv(&secp_ctx, xpriv, keyset_info);
                        keysets.insert(id, keyset);
                    }
                }
            }
            true => {
                let derivation_path = DerivationPath::from(vec![
                    ChildNumber::from_hardened_idx(0).expect("0 is a valid index")
                ]);
                let (keyset, keyset_info) =
                    create_new_keyset(&secp_ctx, xpriv, derivation_path, CurrencyUnit::Sat, 64);
                let id = keyset_info.id;
                localstore.add_keyset_info(keyset_info).await?;
                localstore.add_active_keyset(CurrencyUnit::Sat, id).await?;
                keysets.insert(id, keyset);
            }
        }

        let mint_url = UncheckedUrl::from(mint_url);

        Ok(Self {
            mint_url,
            keysets: Arc::new(RwLock::new(keysets)),
            secp_ctx,
            xpriv,
            localstore,
            fee_reserve: FeeReserve {
                min_fee_reserve,
                percent_fee_reserve,
            },
            mint_info,
        })
    }

    /// Set Mint Url
    pub fn set_mint_url(&mut self, mint_url: UncheckedUrl) {
        self.mint_url = mint_url;
    }

    /// Get Mint Url
    pub fn get_mint_url(&self) -> &UncheckedUrl {
        &self.mint_url
    }

    /// Set Mint Info
    pub fn set_mint_info(&mut self, mint_info: MintInfo) {
        self.mint_info = mint_info;
    }

    /// Get Mint Info
    pub fn mint_info(&self) -> &MintInfo {
        &self.mint_info
    }

    /// New mint quote
    pub async fn new_mint_quote(
        &self,
        mint_url: UncheckedUrl,
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        expiry: u64,
    ) -> Result<MintQuote, Error> {
        let quote = MintQuote::new(mint_url, request, unit, amount, expiry);

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Check mint quote
    pub async fn check_mint_quote(&self, quote_id: &str) -> Result<MintQuoteBolt11Response, Error> {
        let quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let paid = quote.state == MintQuoteState::Paid;

        Ok(MintQuoteBolt11Response {
            quote: quote.id,
            request: quote.request,
            paid: Some(paid),
            state: quote.state,
            expiry: Some(quote.expiry),
        })
    }

    /// Update mint quote
    pub async fn update_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        self.localstore.add_mint_quote(quote).await?;
        Ok(())
    }

    /// Get mint quotes
    pub async fn mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let quotes = self.localstore.get_mint_quotes().await?;
        Ok(quotes)
    }

    /// Get pending mint quotes
    pub async fn get_pending_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let mint_quotes = self.localstore.get_mint_quotes().await?;

        Ok(mint_quotes
            .into_iter()
            .filter(|p| p.state == MintQuoteState::Pending)
            .collect())
    }

    /// Remove mint quote
    pub async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.localstore.remove_mint_quote(quote_id).await?;

        Ok(())
    }

    /// New melt quote
    pub async fn new_melt_quote(
        &self,
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        fee_reserve: Amount,
        expiry: u64,
    ) -> Result<MeltQuote, Error> {
        let quote = MeltQuote::new(request, unit, amount, fee_reserve, expiry);

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Check melt quote status
    pub async fn check_melt_quote(&self, quote_id: &str) -> Result<MeltQuoteBolt11Response, Error> {
        let quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        Ok(MeltQuoteBolt11Response {
            quote: quote.id,
            paid: Some(quote.state == QuoteState::Paid),
            state: quote.state,
            expiry: quote.expiry,
            amount: quote.amount,
            fee_reserve: quote.fee_reserve,
            payment_preimage: quote.payment_preimage,
            change: None,
        })
    }

    /// Update melt quote
    pub async fn update_melt_quote(&self, quote: MeltQuote) -> Result<(), Error> {
        self.localstore.add_melt_quote(quote).await?;
        Ok(())
    }

    /// Get melt quotes
    pub async fn melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes)
    }

    /// Remove melt quote
    pub async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.localstore.remove_melt_quote(quote_id).await?;

        Ok(())
    }

    /// Retrieve the public keys of the active keyset for distribution to
    /// wallet clients
    pub async fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.ensure_keyset_loaded(keyset_id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(keyset_id).ok_or(Error::UnknownKeySet)?.clone();
        Ok(KeysResponse {
            keysets: vec![keyset.into()],
        })
    }

    /// Retrieve the public keys of the active keyset for distribution to
    /// wallet clients
    pub async fn pubkeys(&self) -> Result<KeysResponse, Error> {
        let keyset_infos = self.localstore.get_keyset_infos().await?;
        for keyset_info in keyset_infos {
            self.ensure_keyset_loaded(&keyset_info.id).await?;
        }
        let keysets = self.keysets.read().await;
        Ok(KeysResponse {
            keysets: keysets.values().map(|k| k.clone().into()).collect(),
        })
    }

    /// Return a list of all supported keysets
    pub async fn keysets(&self) -> Result<KeysetResponse, Error> {
        let keysets = self.localstore.get_keyset_infos().await?;
        let active_keysets: HashSet<Id> = self
            .localstore
            .get_active_keysets()
            .await?
            .values()
            .cloned()
            .collect();

        let keysets = keysets
            .into_iter()
            .map(|k| KeySetInfo {
                id: k.id,
                unit: k.unit,
                active: active_keysets.contains(&k.id),
            })
            .collect();

        Ok(KeysetResponse { keysets })
    }

    /// Get keysets
    pub async fn keyset(&self, id: &Id) -> Result<Option<KeySet>, Error> {
        self.ensure_keyset_loaded(id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(id).map(|k| k.clone().into());
        Ok(keyset)
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    pub async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        derivation_path: DerivationPath,
        max_order: u8,
    ) -> Result<(), Error> {
        let (keyset, keyset_info) = create_new_keyset(
            &self.secp_ctx,
            self.xpriv,
            derivation_path,
            unit.clone(),
            max_order,
        );
        let id = keyset_info.id;
        self.localstore.add_keyset_info(keyset_info).await?;
        self.localstore.add_active_keyset(unit, id).await?;

        let mut keysets = self.keysets.write().await;
        keysets.insert(id, keyset);

        Ok(())
    }

    /// Process mint request
    pub async fn process_mint_request(
        &self,
        mint_request: nut04::MintBolt11Request,
    ) -> Result<nut04::MintBolt11Response, Error> {
        let state = self
            .localstore
            .update_mint_quote_state(&mint_request.quote, MintQuoteState::Pending)
            .await?;

        match state {
            MintQuoteState::Unpaid => {
                return Err(Error::UnpaidQuote);
            }
            MintQuoteState::Pending => {
                return Err(Error::PendingQuote);
            }
            MintQuoteState::Issued => {
                return Err(Error::IssuedQuote);
            }
            MintQuoteState::Paid => (),
        }

        for blinded_message in &mint_request.outputs {
            if self
                .localstore
                .get_blinded_signature(&blinded_message.blinded_secret)
                .await?
                .is_some()
            {
                tracing::error!(
                    "Output has already been signed: {}",
                    blinded_message.blinded_secret
                );
                return Err(Error::BlindedMessageAlreadySigned);
            }
        }

        let mut blind_signatures = Vec::with_capacity(mint_request.outputs.len());

        for blinded_message in mint_request.outputs.into_iter() {
            let blinded_signature = self.blind_sign(&blinded_message).await?;
            self.localstore
                .add_blinded_signature(blinded_message.blinded_secret, blinded_signature.clone())
                .await?;
            blind_signatures.push(blinded_signature);
        }

        self.localstore
            .update_mint_quote_state(&mint_request.quote, MintQuoteState::Issued)
            .await?;

        Ok(nut04::MintBolt11Response {
            signatures: blind_signatures,
        })
    }

    /// Blind Sign
    pub async fn blind_sign(
        &self,
        blinded_message: &BlindedMessage,
    ) -> Result<BlindSignature, Error> {
        let BlindedMessage {
            amount,
            blinded_secret,
            keyset_id,
            ..
        } = blinded_message;
        self.ensure_keyset_loaded(keyset_id).await?;

        let keyset_info = self
            .localstore
            .get_keyset_info(keyset_id)
            .await?
            .ok_or(Error::UnknownKeySet)?;

        let active = self
            .localstore
            .get_active_keyset_id(&keyset_info.unit)
            .await?
            .ok_or(Error::InactiveKeyset)?;

        // Check that the keyset is active and should be used to sign
        if keyset_info.id.ne(&active) {
            return Err(Error::InactiveKeyset);
        }

        let keysets = self.keysets.read().await;
        let keyset = keysets.get(keyset_id).ok_or(Error::UnknownKeySet)?;
        let Some(key_pair) = keyset.keys.get(amount) else {
            // No key for amount
            return Err(Error::AmountKey);
        };

        let c = sign_message(&key_pair.secret_key, blinded_secret)?;

        let blinded_signature = BlindSignature::new(
            *amount,
            c,
            keyset_info.id,
            &blinded_message.blinded_secret,
            key_pair.secret_key.clone(),
        )?;

        Ok(blinded_signature)
    }

    /// Process Swap
    pub async fn process_swap_request(
        &self,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        for blinded_message in &swap_request.outputs {
            if self
                .localstore
                .get_blinded_signature(&blinded_message.blinded_secret)
                .await?
                .is_some()
            {
                tracing::error!(
                    "Output has already been signed: {}",
                    blinded_message.blinded_secret
                );
                return Err(Error::BlindedMessageAlreadySigned);
            }
        }

        let proofs_total = swap_request.input_amount();

        let output_total = swap_request.output_amount();

        if proofs_total != output_total {
            return Err(Error::Amount);
        }

        let proof_count = swap_request.inputs.len();

        let secrets: HashSet<[u8; 33]> = swap_request
            .inputs
            .iter()
            .flat_map(|p| hash_to_curve(&p.secret.to_bytes()))
            .map(|p| p.to_bytes())
            .collect();

        // Check that there are no duplicate proofs in request
        if secrets.len().ne(&proof_count) {
            return Err(Error::DuplicateProofs);
        }

        for proof in &swap_request.inputs {
            self.verify_proof(proof).await?
        }

        let input_keyset_ids: HashSet<Id> =
            swap_request.inputs.iter().map(|p| p.keyset_id).collect();

        let mut keyset_units = HashSet::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset_info(&id)
                .await?
                .ok_or(Error::UnknownKeySet)?;
            keyset_units.insert(keyset.unit);
        }

        let output_keyset_ids: HashSet<Id> =
            swap_request.outputs.iter().map(|p| p.keyset_id).collect();

        for id in &output_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset_info(id)
                .await?
                .ok_or(Error::UnknownKeySet)?;

            keyset_units.insert(keyset.unit);
        }

        // Check that all proofs are the same unit
        // in the future it maybe possible to support multiple units but unsupported for
        // now
        if keyset_units.len().gt(&1) {
            tracing::error!("Only one unit is allowed in request: {:?}", keyset_units);
            return Err(Error::MultipleUnits);
        }

        let (sig_flag, pubkeys) = enforce_sig_flag(swap_request.inputs.clone());

        if sig_flag.eq(&SigFlag::SigAll) {
            let pubkeys = pubkeys.into_iter().collect();
            for blinded_messaage in &swap_request.outputs {
                blinded_messaage.verify_p2pk(&pubkeys, 1)?;
            }
        }

        self.localstore
            .add_spent_proofs(swap_request.inputs)
            .await?;

        let mut promises = Vec::with_capacity(swap_request.outputs.len());

        for blinded_message in swap_request.outputs {
            let blinded_signature = self.blind_sign(&blinded_message).await?;
            self.localstore
                .add_blinded_signature(blinded_message.blinded_secret, blinded_signature.clone())
                .await?;
            promises.push(blinded_signature);
        }

        Ok(SwapResponse::new(promises))
    }

    /// Verify [`Proof`] meets conditions and is signed
    pub async fn verify_proof(&self, proof: &Proof) -> Result<(), Error> {
        // Check if secret is a nut10 secret with conditions
        if let Ok(secret) =
            <&crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(&proof.secret)
        {
            // Checks and verifes known secret kinds.
            // If it is an unknown secret kind it will be treated as a normal secret.
            // Spending conditions will **not** be check. It is up to the wallet to ensure
            // only supported secret kinds are used as there is no way for the mint to enforce
            // only signing supported secrets as they are blinded at that point.
            match secret.kind {
                Kind::P2PK => {
                    proof.verify_p2pk()?;
                }
                Kind::HTLC => {
                    proof.verify_htlc()?;
                }
            }
        }

        let y: PublicKey = hash_to_curve(&proof.secret.to_bytes())?;

        if self.localstore.get_spent_proof_by_y(&y).await?.is_some() {
            return Err(Error::TokenAlreadySpent);
        }

        if self.localstore.get_pending_proof_by_y(&y).await?.is_some() {
            return Err(Error::TokenPending);
        }

        self.ensure_keyset_loaded(&proof.keyset_id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(&proof.keyset_id).ok_or(Error::UnknownKeySet)?;
        let Some(keypair) = keyset.keys.get(&proof.amount) else {
            return Err(Error::AmountKey);
        };

        verify_message(&keypair.secret_key, proof.c, proof.secret.as_bytes())?;

        Ok(())
    }

    /// Check state
    pub async fn check_state(
        &self,
        check_state: &CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let mut states = Vec::with_capacity(check_state.ys.len());

        for y in &check_state.ys {
            let state = if self.localstore.get_spent_proof_by_y(y).await?.is_some() {
                State::Spent
            } else if self.localstore.get_pending_proof_by_y(y).await?.is_some() {
                State::Pending
            } else {
                State::Unspent
            };

            states.push(ProofState {
                y: *y,
                state,
                witness: None,
            })
        }
        Ok(CheckStateResponse { states })
    }

    /// Verify melt request is valid
    pub async fn verify_melt_request(
        &self,
        melt_request: &MeltBolt11Request,
    ) -> Result<MeltQuote, Error> {
        for proof in &melt_request.inputs {
            self.verify_proof(proof).await?;
        }

        let state = self
            .localstore
            .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Pending)
            .await?;

        match state {
            MeltQuoteState::Unpaid => (),
            MeltQuoteState::Pending => {
                return Err(Error::PendingQuote);
            }
            MeltQuoteState::Paid => {
                return Err(Error::PaidQuote);
            }
        }

        let quote = self
            .localstore
            .get_melt_quote(&melt_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let proofs_total = melt_request.proofs_amount();

        let required_total = quote.amount + quote.fee_reserve;

        if proofs_total < required_total {
            tracing::debug!(
                "Insufficient Proofs: Got: {}, Required: {}",
                proofs_total,
                required_total
            );
            return Err(Error::Amount);
        }

        let input_keyset_ids: HashSet<Id> =
            melt_request.inputs.iter().map(|p| p.keyset_id).collect();

        let mut keyset_units = HashSet::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            let keyset = self
                .localstore
                .get_keyset_info(&id)
                .await?
                .ok_or(Error::UnknownKeySet)?;
            keyset_units.insert(keyset.unit);
        }

        if let Some(outputs) = &melt_request.outputs {
            let (sig_flag, pubkeys) = enforce_sig_flag(melt_request.inputs.clone());

            if sig_flag.eq(&SigFlag::SigAll) {
                let pubkeys = pubkeys.into_iter().collect();
                for blinded_messaage in outputs {
                    blinded_messaage.verify_p2pk(&pubkeys, 1)?;
                }
            }

            let output_keysets_ids: HashSet<Id> = outputs.iter().map(|b| b.keyset_id).collect();
            for id in output_keysets_ids {
                let keyset = self
                    .localstore
                    .get_keyset_info(&id)
                    .await?
                    .ok_or(Error::UnknownKeySet)?;

                // Get the active keyset for the unit
                let active_keyset_id = self
                    .localstore
                    .get_active_keyset_id(&keyset.unit)
                    .await?
                    .ok_or(Error::InactiveKeyset)?;

                // Check output is for current active keyset
                if id.ne(&active_keyset_id) {
                    return Err(Error::InactiveKeyset);
                }
                keyset_units.insert(keyset.unit);
            }
        }

        // Check that all input and output proofs are the same unit
        if keyset_units.len().gt(&1) {
            return Err(Error::MultipleUnits);
        }

        let secrets: HashSet<[u8; 33]> = melt_request
            .inputs
            .iter()
            .flat_map(|p| hash_to_curve(&p.secret.to_bytes()))
            .map(|p| p.to_bytes())
            .collect();

        // Ensure proofs are unique and not being double spent
        if melt_request.inputs.len().ne(&secrets.len()) {
            return Err(Error::DuplicateProofs);
        }

        // Add proofs to pending
        self.localstore
            .add_pending_proofs(melt_request.inputs.clone())
            .await?;

        tracing::debug!("Verified melt quote: {}", melt_request.quote);
        Ok(quote)
    }

    /// Process melt request marking [`Proofs`] as spent
    /// The melt request must be verifyed using [`Self::verify_melt_request`] before calling [`Self::process_melt_request`]
    pub async fn process_melt_request(
        &self,
        melt_request: &MeltBolt11Request,
        payment_preimage: Option<String>,
        total_spent: Amount,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        tracing::debug!("Processing melt quote: {}", melt_request.quote);
        let quote = self
            .localstore
            .get_melt_quote(&melt_request.quote)
            .await?
            .ok_or(Error::UnknownQuote)?;

        if let Some(outputs) = &melt_request.outputs {
            for blinded_message in outputs {
                if self
                    .localstore
                    .get_blinded_signature(&blinded_message.blinded_secret)
                    .await?
                    .is_some()
                {
                    tracing::error!(
                        "Output has already been signed: {}",
                        blinded_message.blinded_secret
                    );
                    return Err(Error::BlindedMessageAlreadySigned);
                }
            }
        }

        self.localstore
            .add_spent_proofs(melt_request.inputs.clone())
            .await?;

        let mut change = None;

        if let Some(outputs) = melt_request.outputs.clone() {
            let change_target = melt_request.proofs_amount() - total_spent;
            let mut amounts = change_target.split();
            let mut change_sigs = Vec::with_capacity(amounts.len());

            if outputs.len().lt(&amounts.len()) {
                tracing::debug!(
                    "Providing change requires {} blinded messages, but only {} provided",
                    amounts.len(),
                    outputs.len()
                );

                // In the case that not enough outputs are provided to return all change
                // Reverse sort the amounts so that the most amount of change possible is
                // returned. The rest is burnt
                amounts.sort_by(|a, b| b.cmp(a));
            }

            for (amount, blinded_message) in amounts.iter().zip(outputs) {
                let mut blinded_message = blinded_message;
                blinded_message.amount = *amount;

                let blinded_signature = self.blind_sign(&blinded_message).await?;
                self.localstore
                    .add_blinded_signature(
                        blinded_message.blinded_secret,
                        blinded_signature.clone(),
                    )
                    .await?;
                change_sigs.push(blinded_signature)
            }

            change = Some(change_sigs);
        } else {
            tracing::info!(
                "No change outputs provided. Burnt: {:?} sats",
                (melt_request.proofs_amount() - total_spent)
            );
        }

        self.localstore
            .remove_pending_proofs(melt_request.inputs.iter().map(|p| &p.secret).collect())
            .await?;

        self.localstore
            .update_melt_quote_state(&melt_request.quote, MeltQuoteState::Paid)
            .await?;

        Ok(MeltQuoteBolt11Response {
            amount: quote.amount,
            paid: Some(true),
            payment_preimage,
            change,
            quote: quote.id,
            fee_reserve: quote.fee_reserve,
            state: QuoteState::Paid,
            expiry: quote.expiry,
        })
    }

    /// Restore
    pub async fn restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        let output_len = request.outputs.len();

        let mut outputs = Vec::with_capacity(output_len);
        let mut signatures = Vec::with_capacity(output_len);

        let blinded_message: Vec<PublicKey> =
            request.outputs.iter().map(|b| b.blinded_secret).collect();

        let blinded_signatures = self
            .localstore
            .get_blinded_signatures(blinded_message)
            .await?;

        assert_eq!(blinded_signatures.len(), output_len);

        for (blinded_message, blinded_signature) in
            request.outputs.into_iter().zip(blinded_signatures)
        {
            if let Some(blinded_signature) = blinded_signature {
                outputs.push(blinded_message);
                signatures.push(blinded_signature);
            }
        }

        Ok(RestoreResponse {
            outputs,
            signatures,
        })
    }

    /// Ensure Keyset is loaded in mint
    pub async fn ensure_keyset_loaded(&self, id: &Id) -> Result<(), Error> {
        let keysets = self.keysets.read().await;
        if keysets.contains_key(id) {
            return Ok(());
        }
        drop(keysets);

        let keyset_info = self
            .localstore
            .get_keyset_info(id)
            .await?
            .ok_or(Error::UnknownKeySet)?;
        let id = keyset_info.id;
        let mut keysets = self.keysets.write().await;
        keysets.insert(id, self.generate_keyset(keyset_info));
        Ok(())
    }

    /// Generate [`MintKeySet`] from [`MintKeySetInfo`]
    pub fn generate_keyset(&self, keyset_info: MintKeySetInfo) -> MintKeySet {
        MintKeySet::generate_from_xpriv(&self.secp_ctx, self.xpriv, keyset_info)
    }
}

/// Mint Fee Reserve
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeeReserve {
    /// Absolute expected min fee
    pub min_fee_reserve: Amount,
    /// Percentage expected fee
    pub percent_fee_reserve: f32,
}

/// Mint Keyset Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeySetInfo {
    /// Keyset [`Id`]
    pub id: Id,
    /// Keyset [`CurrencyUnit`]
    pub unit: CurrencyUnit,
    /// Keyset active or inactive
    /// Mint will only issue new [`BlindSignature`] on active keysets
    pub active: bool,
    /// Starting unix time Keyset is valid from
    pub valid_from: u64,
    /// When the Keyset is valid to
    /// This is not shown to the wallet and can only be used internally
    pub valid_to: Option<u64>,
    /// [`DerivationPath`] of Keyset
    pub derivation_path: DerivationPath,
    /// Max order of keyset
    pub max_order: u8,
}

impl From<MintKeySetInfo> for KeySetInfo {
    fn from(keyset_info: MintKeySetInfo) -> Self {
        Self {
            id: keyset_info.id,
            unit: keyset_info.unit,
            active: keyset_info.active,
        }
    }
}

/// Generate new [`MintKeySetInfo`] from path
fn create_new_keyset<C: secp256k1::Signing>(
    secp: &secp256k1::Secp256k1<C>,
    xpriv: ExtendedPrivKey,
    derivation_path: DerivationPath,
    unit: CurrencyUnit,
    max_order: u8,
) -> (MintKeySet, MintKeySetInfo) {
    let keyset = MintKeySet::generate(
        secp,
        xpriv
            .derive_priv(secp, &derivation_path)
            .expect("RNG busted"),
        unit,
        max_order,
    );
    let keyset_info = MintKeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: true,
        valid_from: unix_time(),
        valid_to: None,
        derivation_path,
        max_order,
    };
    (keyset, keyset_info)
}
