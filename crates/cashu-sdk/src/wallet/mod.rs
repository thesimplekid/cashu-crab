//! Cashu Wallet
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use bip39::Mnemonic;
use cashu::dhke::{construct_proofs, unblind_message};
#[cfg(feature = "nut07")]
use cashu::nuts::nut07::ProofState;
use cashu::nuts::nut07::State;
#[cfg(feature = "nut09")]
use cashu::nuts::nut09::RestoreRequest;
use cashu::nuts::nut11::SigningKey;
#[cfg(feature = "nut07")]
use cashu::nuts::PublicKey;
use cashu::nuts::{
    BlindedSignature, CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, P2PKConditions,
    PreMintSecrets, PreSwap, Proof, Proofs, SigFlag, SwapRequest, Token,
};
use cashu::types::{MeltQuote, Melted, MintQuote};
use cashu::url::UncheckedUrl;
use cashu::{Amount, Bolt11Invoice};
use localstore::LocalStore;
use thiserror::Error;
use tracing::{debug, warn};

use crate::client::Client;
use crate::utils::unix_time;

pub mod localstore;

#[derive(Debug, Error)]
pub enum Error {
    /// Insufficient Funds
    #[error("Insufficient Funds")]
    InsufficientFunds,
    #[error("`{0}`")]
    CashuWallet(#[from] cashu::error::wallet::Error),
    #[error("`{0}`")]
    Client(#[from] crate::client::Error),
    /// Cashu Url Error
    #[error("`{0}`")]
    CashuUrl(#[from] cashu::url::Error),
    #[error("Quote Expired")]
    QuoteExpired,
    #[error("Quote Unknown")]
    QuoteUnknown,
    #[error("No active keyset")]
    NoActiveKeyset,
    #[error("`{0}`")]
    LocalStore(#[from] localstore::Error),
    #[error("`{0}`")]
    Cashu(#[from] cashu::error::Error),
    #[error("Could not verify Dleq")]
    CouldNotVerifyDleq,
    #[error("Unknown Key")]
    UnknownKey,
    #[error("`{0}`")]
    Custom(String),
}

#[derive(Clone, Debug)]
pub struct Wallet<C: Client, L: LocalStore> {
    pub client: C,
    localstore: L,
    mnemonic: Option<Mnemonic>,
}

impl<C: Client, L: LocalStore> Wallet<C, L> {
    pub async fn new(client: C, localstore: L, mnemonic: Option<Mnemonic>) -> Self {
        Self {
            mnemonic,
            client,
            localstore,
        }
    }

    /// Back up seed
    pub fn mnemonic(&self) -> Option<Mnemonic> {
        self.mnemonic.clone()
    }

    pub async fn mint_balances(&self) -> Result<HashMap<UncheckedUrl, Amount>, Error> {
        let mints = self.localstore.get_mints().await?;

        let mut balances = HashMap::new();

        for (mint, _) in mints {
            if let Some(proofs) = self.localstore.get_proofs(mint.clone()).await? {
                let amount = proofs.iter().map(|p| p.amount).sum();

                balances.insert(mint, amount);
            } else {
                balances.insert(mint, Amount::ZERO);
            }
        }

        Ok(balances)
    }

    pub async fn get_proofs(&self, mint_url: UncheckedUrl) -> Result<Option<Proofs>, Error> {
        Ok(self.localstore.get_proofs(mint_url).await?)
    }

    pub async fn add_mint(&self, mint_url: UncheckedUrl) -> Result<Option<MintInfo>, Error> {
        let mint_info = match self
            .client
            .get_mint_info(mint_url.clone().try_into()?)
            .await
        {
            Ok(mint_info) => Some(mint_info),
            Err(err) => {
                warn!("Could not get mint info {}", err);
                None
            }
        };

        self.localstore
            .add_mint(mint_url, mint_info.clone())
            .await?;

        Ok(mint_info)
    }

    pub async fn get_keyset_keys(
        &self,
        mint_url: &UncheckedUrl,
        keyset_id: Id,
    ) -> Result<Keys, Error> {
        let keys = if let Some(keys) = self.localstore.get_keys(&keyset_id).await? {
            keys
        } else {
            let keys = self
                .client
                .get_mint_keyset(mint_url.try_into()?, keyset_id)
                .await?;

            self.localstore.add_keys(keys.keys.clone()).await?;

            keys.keys
        };

        Ok(keys)
    }

    pub async fn get_mint_keysets(
        &self,
        mint_url: &UncheckedUrl,
    ) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self.client.get_mint_keysets(mint_url.try_into()?).await?;

        self.localstore
            .add_mint_keysets(mint_url.clone(), keysets.keysets.clone())
            .await?;

        Ok(keysets.keysets)
    }

    pub async fn get_active_mint_keys(
        &self,
        mint_url: &UncheckedUrl,
    ) -> Result<Vec<KeySet>, Error> {
        let keysets = self.client.get_mint_keys(mint_url.try_into()?).await?;

        for keyset in keysets.clone() {
            self.localstore.add_keys(keyset.keys).await?;
        }

        let k = self.client.get_mint_keysets(mint_url.try_into()?).await?;

        self.localstore
            .add_mint_keysets(mint_url.clone(), k.keysets)
            .await?;

        Ok(keysets)
    }

    /// Check if a proof is spent
    #[cfg(feature = "nut07")]
    pub async fn check_proofs_spent(
        &self,
        mint_url: UncheckedUrl,
        proofs: Proofs,
    ) -> Result<Vec<ProofState>, Error> {
        use cashu::dhke::hash_to_curve;

        let spendable = self
            .client
            .post_check_state(
                mint_url.try_into()?,
                proofs
                    .clone()
                    .into_iter()
                    // Find Y for the secret
                    .flat_map(|p| hash_to_curve(&p.secret.to_bytes()))
                    .map(|y| y.into())
                    .collect::<Vec<PublicKey>>()
                    .clone(),
            )
            .await?;

        // Separate proofs in spent and unspent based on mint response

        Ok(spendable.states)
    }

    /// Mint Quote
    pub async fn mint_quote(
        &mut self,
        mint_url: UncheckedUrl,
        amount: Amount,
        unit: CurrencyUnit,
    ) -> Result<MintQuote, Error> {
        let quote_res = self
            .client
            .post_mint_quote(mint_url.try_into()?, amount, unit.clone())
            .await?;

        let quote = MintQuote {
            id: quote_res.quote.clone(),
            amount,
            unit: unit.clone(),
            request: quote_res.request,
            paid: quote_res.paid,
            expiry: quote_res.expiry,
        };

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    async fn active_mint_keyset(
        &mut self,
        mint_url: &UncheckedUrl,
        unit: &CurrencyUnit,
    ) -> Result<Id, Error> {
        if let Some(keysets) = self.localstore.get_mint_keysets(mint_url.clone()).await? {
            for keyset in keysets {
                if keyset.unit.eq(unit) && keyset.active {
                    return Ok(keyset.id);
                }
            }
        }

        let keysets = self.client.get_mint_keysets(mint_url.try_into()?).await?;

        self.localstore
            .add_mint_keysets(
                mint_url.clone(),
                keysets.keysets.clone().into_iter().collect(),
            )
            .await?;
        for keyset in &keysets.keysets {
            if keyset.unit.eq(unit) && keyset.active {
                return Ok(keyset.id);
            }
        }

        Err(Error::NoActiveKeyset)
    }

    async fn active_keys(
        &mut self,
        mint_url: &UncheckedUrl,
        unit: &CurrencyUnit,
    ) -> Result<Option<Keys>, Error> {
        let active_keyset_id = self.active_mint_keyset(mint_url, unit).await?;

        let keys;

        if let Some(k) = self.localstore.get_keys(&active_keyset_id).await? {
            keys = Some(k.clone())
        } else {
            let keyset = self
                .client
                .get_mint_keyset(mint_url.try_into()?, active_keyset_id)
                .await?;

            self.localstore.add_keys(keyset.keys.clone()).await?;
            keys = Some(keyset.keys);
        }

        Ok(keys)
    }

    /// Mint
    pub async fn mint(&mut self, mint_url: UncheckedUrl, quote_id: &str) -> Result<Amount, Error> {
        // Check that mint is in store of mints
        if self.localstore.get_mint(mint_url.clone()).await?.is_none() {
            self.add_mint(mint_url.clone()).await?;
        }

        let quote_info = self.localstore.get_mint_quote(quote_id).await?;

        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) && quote.expiry.ne(&0) {
                return Err(Error::QuoteExpired);
            }

            quote.clone()
        } else {
            return Err(Error::QuoteUnknown);
        };

        let active_keyset_id = self.active_mint_keyset(&mint_url, &quote_info.unit).await?;

        let mut counter = None;

        let premint_secrets = match &self.mnemonic {
            Some(mnemonic) => {
                let count = self
                    .localstore
                    .get_keyset_counter(&active_keyset_id)
                    .await?;

                let count = if let Some(count) = count {
                    count + 1
                } else {
                    0
                };

                counter = Some(count);
                PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    mnemonic,
                    quote_info.amount,
                    false,
                )?
            }
            None => PreMintSecrets::random(active_keyset_id, quote_info.amount)?,
        };

        let mint_res = self
            .client
            .post_mint(
                mint_url.clone().try_into()?,
                quote_id,
                premint_secrets.clone(),
            )
            .await?;

        let keys = self.get_keyset_keys(&mint_url, active_keyset_id).await?;

        // Verify the signature DLEQ is valid
        #[cfg(feature = "nut12")]
        {
            for (sig, premint) in mint_res.signatures.iter().zip(&premint_secrets.secrets) {
                let keys = self.get_keyset_keys(&mint_url, sig.keyset_id).await?;
                let key = keys.amount_key(sig.amount).ok_or(Error::UnknownKey)?;
                match sig.verify_dleq(&key, &premint.blinded_message.b) {
                    Ok(_) => (),
                    Err(cashu::nuts::nut12::Error::MissingDleqProof) => (),
                    Err(_) => return Err(Error::CouldNotVerifyDleq),
                }
            }
        }

        let proofs = construct_proofs(
            mint_res.signatures,
            premint_secrets.rs(),
            premint_secrets.secrets(),
            &keys,
        )?;

        let minted_amount = proofs.iter().map(|p| p.amount).sum();

        // Remove filled quote from store
        self.localstore.remove_mint_quote(&quote_info.id).await?;

        // Update counter for keyset
        if counter.is_some() {
            self.localstore
                .increment_keyset_counter(&active_keyset_id, proofs.len() as u64)
                .await?;
        }

        // Add new proofs to store
        self.localstore.add_proofs(mint_url, proofs).await?;

        Ok(minted_amount)
    }

    /// Receive
    pub async fn receive(&mut self, encoded_token: &str) -> Result<(), Error> {
        let token_data = Token::from_str(encoded_token)?;

        let unit = token_data.unit.unwrap_or_default();

        // Verify the signature DLEQ is valid
        // Verify that all proofs in the token have a vlid DLEQ proof if one is supplied
        #[cfg(feature = "nut12")]
        {
            for mint_proof in &token_data.token {
                let mint_url = &mint_proof.mint;
                let proofs = &mint_proof.proofs;

                for proof in proofs {
                    let keys = self.get_keyset_keys(mint_url, proof.keyset_id).await?;
                    let key = keys.amount_key(proof.amount).ok_or(Error::UnknownKey)?;
                    match proof.verify_dleq(&key) {
                        Ok(_) => continue,
                        Err(cashu::nuts::nut12::Error::MissingDleqProof) => continue,
                        Err(_) => return Err(Error::CouldNotVerifyDleq),
                    }
                }
            }
        }

        let mut proofs: HashMap<UncheckedUrl, Proofs> = HashMap::new();
        for token in token_data.token {
            if token.proofs.is_empty() {
                continue;
            }

            let active_keyset_id = self.active_mint_keyset(&token.mint, &unit).await?;

            // TODO: if none fetch keyset for mint

            let keys = if let Some(keys) = self.localstore.get_keys(&active_keyset_id).await? {
                keys
            } else {
                self.get_keyset_keys(&token.mint, active_keyset_id).await?;
                self.localstore.get_keys(&active_keyset_id).await?.unwrap()
            };

            // Sum amount of all proofs
            let amount: Amount = token.proofs.iter().map(|p| p.amount).sum();

            let pre_swap = self
                .create_swap(&token.mint, &unit, Some(amount), token.proofs)
                .await?;

            let swap_response = self
                .client
                .post_swap(token.mint.clone().try_into()?, pre_swap.swap_request)
                .await?;

            // Proof to keep
            let p = construct_proofs(
                swap_response.signatures,
                pre_swap.pre_mint_secrets.rs(),
                pre_swap.pre_mint_secrets.secrets(),
                &keys,
            )?;

            if self.mnemonic.is_some() {
                self.localstore
                    .increment_keyset_counter(&active_keyset_id, p.len() as u64)
                    .await?;
            }

            let mint_proofs = proofs.entry(token.mint).or_default();

            mint_proofs.extend(p);
        }

        for (mint, p) in proofs {
            self.add_mint(mint.clone()).await?;
            self.localstore.add_proofs(mint, p).await?;
        }

        Ok(())
    }

    /// Create Swap Payload
    async fn create_swap(
        &mut self,
        mint_url: &UncheckedUrl,
        unit: &CurrencyUnit,
        amount: Option<Amount>,
        proofs: Proofs,
    ) -> Result<PreSwap, Error> {
        let active_keyset_id = self.active_mint_keyset(mint_url, unit).await?;

        // Desired amount is either amount passwed or value of all proof
        let proofs_total = proofs.iter().map(|p| p.amount).sum();

        let desired_amount = amount.unwrap_or(proofs_total);

        let mut counter = None;

        let mut desired_messages = if let Some(mnemonic) = &self.mnemonic {
            let count = self
                .localstore
                .get_keyset_counter(&active_keyset_id)
                .await?;

            let count = if let Some(count) = count {
                count + 1
            } else {
                0
            };

            let premint_secrets = PreMintSecrets::from_seed(
                active_keyset_id,
                count,
                mnemonic,
                desired_amount,
                false,
            )?;

            counter = Some(count + premint_secrets.len() as u64);

            premint_secrets
        } else {
            PreMintSecrets::random(active_keyset_id, desired_amount)?
        };

        if let Some(amt) = amount {
            let change_amount = proofs_total - amt;

            let change_messages = if let (Some(count), Some(mnemonic)) = (counter, &self.mnemonic) {
                PreMintSecrets::from_seed(active_keyset_id, count, mnemonic, change_amount, false)?
            } else {
                PreMintSecrets::random(active_keyset_id, change_amount)?
            };
            // Combine the BlindedMessages totoalling the desired amount with change
            desired_messages.combine(change_messages);
            // Sort the premint secrets to avoid finger printing
            desired_messages.sort_secrets();
        };

        let swap_request = SwapRequest::new(proofs, desired_messages.blinded_messages());

        Ok(PreSwap {
            pre_mint_secrets: desired_messages,
            swap_request,
        })
    }

    pub async fn process_swap_response(
        &self,
        blinded_messages: PreMintSecrets,
        promises: Vec<BlindedSignature>,
    ) -> Result<Proofs, Error> {
        let mut proofs = vec![];

        let mut proof_count: HashMap<Id, u64> = HashMap::new();

        for (promise, premint) in promises.iter().zip(blinded_messages) {
            // Verify the signature DLEQ is valid
            #[cfg(feature = "nut12")]
            {
                let keys = self
                    .localstore
                    .get_keys(&promise.keyset_id)
                    .await?
                    .ok_or(Error::UnknownKey)?;
                let key = keys.amount_key(promise.amount).ok_or(Error::UnknownKey)?;
                match promise.verify_dleq(&key, &premint.blinded_message.b) {
                    Ok(_) => (),
                    Err(cashu::nuts::nut12::Error::MissingDleqProof) => (),
                    Err(_) => return Err(Error::CouldNotVerifyDleq),
                }
            }

            let a = self
                .localstore
                .get_keys(&promise.keyset_id)
                .await?
                .unwrap()
                .amount_key(promise.amount)
                .unwrap()
                .to_owned();

            let blinded_c = promise.c.clone();

            let unblinded_sig = unblind_message(blinded_c, premint.r.into(), a).unwrap();

            let count = proof_count.get(&promise.keyset_id).unwrap_or(&0);
            proof_count.insert(promise.keyset_id, count + 1);

            let proof = Proof::new(
                promise.amount,
                promise.keyset_id,
                premint.secret,
                unblinded_sig,
            );

            proofs.push(proof);
        }

        if self.mnemonic.is_some() {
            for (keyset_id, count) in proof_count {
                self.localstore
                    .increment_keyset_counter(&keyset_id, count)
                    .await?;
            }
        }

        Ok(proofs)
    }

    /// Send
    pub async fn send(
        &mut self,
        mint_url: &UncheckedUrl,
        unit: &CurrencyUnit,
        amount: Amount,
    ) -> Result<Proofs, Error> {
        let proofs = self.select_proofs(mint_url.clone(), unit, amount).await?;

        let pre_swap = self
            .create_swap(mint_url, unit, Some(amount), proofs.clone())
            .await?;

        let swap_response = self
            .client
            .post_swap(mint_url.clone().try_into()?, pre_swap.swap_request)
            .await?;

        let mut keep_proofs = Proofs::new();
        let mut send_proofs = Proofs::new();

        let mut post_swap_proofs = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &self.active_keys(mint_url, unit).await?.unwrap(),
        )?;

        let active_keyset = self.active_mint_keyset(mint_url, unit).await?;

        if self.mnemonic.is_some() {
            self.localstore
                .increment_keyset_counter(&active_keyset, post_swap_proofs.len() as u64)
                .await?;
        }

        post_swap_proofs.reverse();

        for proof in post_swap_proofs {
            if (proof.amount + send_proofs.iter().map(|p| p.amount).sum()).gt(&amount) {
                keep_proofs.push(proof);
            } else {
                send_proofs.push(proof);
            }
        }

        let send_amount: Amount = send_proofs.iter().map(|p| p.amount).sum();

        if send_amount.ne(&amount) {
            warn!(
                "Send amount proofs is {:?} expected {:?}",
                send_amount, amount
            );
        }

        self.localstore
            .remove_proofs(mint_url.clone(), &proofs)
            .await?;

        self.localstore
            .add_pending_proofs(mint_url.clone(), proofs)
            .await?;
        self.localstore
            .add_pending_proofs(mint_url.clone(), send_proofs.clone())
            .await?;

        self.localstore
            .add_proofs(mint_url.clone(), keep_proofs)
            .await?;

        Ok(send_proofs)
    }

    /// Melt Quote
    pub async fn melt_quote(
        &mut self,
        mint_url: UncheckedUrl,
        unit: CurrencyUnit,
        request: String,
    ) -> Result<MeltQuote, Error> {
        let quote_res = self
            .client
            .post_melt_quote(
                mint_url.clone().try_into()?,
                unit.clone(),
                Bolt11Invoice::from_str(&request.clone()).unwrap(),
            )
            .await?;

        let quote = MeltQuote {
            id: quote_res.quote,
            amount: quote_res.amount.into(),
            request,
            unit,
            fee_reserve: quote_res.fee_reserve.into(),
            paid: quote_res.paid,
            expiry: quote_res.expiry,
        };

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }

    // Select proofs
    pub async fn select_proofs(
        &self,
        mint_url: UncheckedUrl,
        unit: &CurrencyUnit,
        amount: Amount,
    ) -> Result<Proofs, Error> {
        let mint_proofs = self
            .localstore
            .get_proofs(mint_url.clone())
            .await?
            .ok_or(Error::InsufficientFunds)?;

        let mint_keysets = self.localstore.get_mint_keysets(mint_url).await?.unwrap();

        let (active, inactive): (HashSet<KeySetInfo>, HashSet<KeySetInfo>) = mint_keysets
            .into_iter()
            .filter(|p| p.unit.eq(unit))
            .partition(|x| x.active);

        let active: HashSet<Id> = active.iter().map(|k| k.id).collect();
        let inactive: HashSet<Id> = inactive.iter().map(|k| k.id).collect();

        let mut active_proofs: Proofs = Vec::new();
        let mut inactive_proofs: Proofs = Vec::new();

        for proof in mint_proofs {
            if active.contains(&proof.keyset_id) {
                active_proofs.push(proof);
            } else if inactive.contains(&proof.keyset_id) {
                inactive_proofs.push(proof);
            }
        }

        active_proofs.reverse();
        inactive_proofs.reverse();

        inactive_proofs.append(&mut active_proofs);

        let proofs = inactive_proofs;

        let mut selected_proofs: Proofs = Vec::new();

        for proof in proofs {
            if selected_proofs.iter().map(|p| p.amount).sum::<Amount>() < amount {
                selected_proofs.push(proof);
            }
        }

        if selected_proofs.iter().map(|p| p.amount).sum::<Amount>() < amount {
            return Err(Error::InsufficientFunds);
        }

        Ok(selected_proofs)
    }

    /// Melt
    pub async fn melt(&mut self, mint_url: &UncheckedUrl, quote_id: &str) -> Result<Melted, Error> {
        let quote_info = self.localstore.get_melt_quote(quote_id).await?;

        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) {
                return Err(Error::QuoteExpired);
            }

            quote.clone()
        } else {
            return Err(Error::QuoteUnknown);
        };

        let proofs = self
            .select_proofs(mint_url.clone(), &quote_info.unit, quote_info.amount)
            .await?;

        let proofs_amount = proofs.iter().map(|p| p.amount).sum();

        let mut counter = None;

        let active_keyset_id = self.active_mint_keyset(mint_url, &quote_info.unit).await?;

        let premint_secrets = match &self.mnemonic {
            Some(mnemonic) => {
                let count = self
                    .localstore
                    .get_keyset_counter(&active_keyset_id)
                    .await?;

                let count = if let Some(count) = count {
                    count + 1
                } else {
                    0
                };

                counter = Some(count);
                PreMintSecrets::from_seed(active_keyset_id, count, mnemonic, proofs_amount, true)?
            }
            None => PreMintSecrets::blank(active_keyset_id, proofs_amount)?,
        };

        let melt_response = self
            .client
            .post_melt(
                mint_url.clone().try_into()?,
                quote_id.to_string(),
                proofs.clone(),
                Some(premint_secrets.blinded_messages()),
            )
            .await?;

        let change_proofs = match melt_response.change {
            Some(change) => Some(construct_proofs(
                change,
                premint_secrets.rs(),
                premint_secrets.secrets(),
                &self.active_keys(mint_url, &quote_info.unit).await?.unwrap(),
            )?),
            None => None,
        };

        let melted = Melted {
            paid: true,
            preimage: melt_response.payment_preimage,
            change: change_proofs.clone(),
        };

        if let Some(change_proofs) = change_proofs {
            debug!(
                "Change amount returned from melt: {}",
                change_proofs.iter().map(|p| p.amount).sum::<Amount>()
            );
            // Update counter for keyset
            if counter.is_some() {
                self.localstore
                    .increment_keyset_counter(&active_keyset_id, change_proofs.len() as u64)
                    .await?;
            }

            self.localstore
                .add_proofs(mint_url.clone(), change_proofs)
                .await?;
        }

        self.localstore.remove_melt_quote(&quote_info.id).await?;

        self.localstore
            .remove_proofs(mint_url.clone(), &proofs)
            .await?;

        Ok(melted)
    }

    /// Create P2PK locked proofs
    /// Uses a swap to swap proofs for locked p2pk conditions
    pub async fn send_p2pk(
        &mut self,
        mint_url: &UncheckedUrl,
        unit: &CurrencyUnit,
        amount: Amount,
        conditions: P2PKConditions,
    ) -> Result<Proofs, Error> {
        let input_proofs = self.select_proofs(mint_url.clone(), unit, amount).await?;
        let active_keyset_id = self.active_mint_keyset(mint_url, unit).await?;

        let input_amount: Amount = input_proofs.iter().map(|p| p.amount).sum();
        let change_amount = input_amount - amount;

        let send_premint_secrets =
            PreMintSecrets::with_p2pk_conditions(active_keyset_id, amount, conditions)?;

        let change_premint_secrets = PreMintSecrets::random(active_keyset_id, change_amount)?;
        let mut pre_mint_secrets = send_premint_secrets;
        pre_mint_secrets.combine(change_premint_secrets);

        let swap_request =
            SwapRequest::new(input_proofs.clone(), pre_mint_secrets.blinded_messages());

        let pre_swap = PreSwap {
            pre_mint_secrets,
            swap_request,
        };

        let swap_response = self
            .client
            .post_swap(mint_url.clone().try_into()?, pre_swap.swap_request)
            .await?;

        let post_swap_proofs = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &self.active_keys(mint_url, unit).await?.unwrap(),
        )?;

        let mut send_proofs = vec![];
        let mut change_proofs = vec![];

        for proof in post_swap_proofs {
            let conditions: Result<cashu::nuts::nut10::Secret, _> = (&proof.secret).try_into();
            if conditions.is_ok() {
                send_proofs.push(proof);
            } else {
                change_proofs.push(proof);
            }
        }

        self.localstore
            .remove_proofs(mint_url.clone(), &input_proofs)
            .await?;

        self.localstore
            .add_pending_proofs(mint_url.clone(), input_proofs)
            .await?;
        self.localstore
            .add_pending_proofs(mint_url.clone(), send_proofs.clone())
            .await?;
        self.localstore
            .add_proofs(mint_url.clone(), change_proofs.clone())
            .await?;

        Ok(send_proofs)
    }

    /// Receive p2pk
    pub async fn receive_p2pk(
        &mut self,
        encoded_token: &str,
        signing_keys: Vec<SigningKey>,
    ) -> Result<(), Error> {
        let signing_key = signing_keys[0].clone();
        let pubkey_secret_key: HashMap<String, SigningKey> = signing_keys
            .into_iter()
            .map(|s| (s.public_key().to_string(), s))
            .collect();

        let token_data = Token::from_str(encoded_token)?;

        let unit = token_data.unit.unwrap_or_default();

        let mut received_proofs: HashMap<UncheckedUrl, Proofs> = HashMap::new();
        for token in token_data.token {
            if token.proofs.is_empty() {
                continue;
            }

            let active_keyset_id = self.active_mint_keyset(&token.mint, &unit).await?;

            // TODO: if none fetch keyset for mint

            let keys = self.localstore.get_keys(&active_keyset_id).await?;

            // Sum amount of all proofs
            let amount: Amount = token.proofs.iter().map(|p| p.amount).sum();

            let mut proofs = token.proofs;

            let mut sig_flag = None;

            for proof in &mut proofs {
                // Verify that proof DLEQ is valid
                #[cfg(feature = "nut12")]
                {
                    let keys = self.localstore.get_keys(&proof.keyset_id).await?.unwrap();
                    let key = keys.amount_key(proof.amount).unwrap();
                    proof.verify_dleq(&key).unwrap();
                }

                if let Ok(secret) =
                    <cashu::secret::Secret as TryInto<cashu::nuts::nut10::Secret>>::try_into(
                        proof.secret.clone(),
                    )
                {
                    let conditions: Result<P2PKConditions, _> = secret.try_into();
                    if let Ok(conditions) = conditions {
                        let pubkeys = conditions.pubkeys;

                        for pubkey in pubkeys {
                            if let Some(signing) = pubkey_secret_key.get(&pubkey.to_string()) {
                                proof.sign_p2pk(signing.clone())?;
                            }
                        }

                        sig_flag = Some(conditions.sig_flag);
                    }
                }
            }

            let mut pre_swap = self
                .create_swap(&token.mint, &unit, Some(amount), proofs)
                .await?;

            if let Some(sigflag) = sig_flag {
                if sigflag.eq(&SigFlag::SigAll) {
                    for blinded_message in &mut pre_swap.swap_request.outputs {
                        blinded_message.sign_p2pk(signing_key.clone()).unwrap();
                    }
                }
            }

            let swap_response = self
                .client
                .post_swap(token.mint.clone().try_into()?, pre_swap.swap_request)
                .await?;

            // Proof to keep
            let p = construct_proofs(
                swap_response.signatures,
                pre_swap.pre_mint_secrets.rs(),
                pre_swap.pre_mint_secrets.secrets(),
                &keys.unwrap(),
            )?;
            let mint_proofs = received_proofs.entry(token.mint).or_default();

            if self.mnemonic.is_some() {
                self.localstore
                    .increment_keyset_counter(&active_keyset_id, p.len() as u64)
                    .await?;
            }

            mint_proofs.extend(p);
        }

        for (mint, proofs) in received_proofs {
            self.localstore.add_proofs(mint, proofs).await?;
        }

        Ok(())
    }

    pub fn proofs_to_token(
        &self,
        mint_url: UncheckedUrl,
        proofs: Proofs,
        memo: Option<String>,
        unit: Option<CurrencyUnit>,
    ) -> Result<String, Error> {
        Ok(Token::new(mint_url, proofs, memo, unit)?.to_string())
    }

    pub async fn restore(&mut self, mint_url: UncheckedUrl) -> Result<Amount, Error> {
        // Check that mint is in store of mints
        if self.localstore.get_mint(mint_url.clone()).await?.is_none() {
            self.add_mint(mint_url.clone()).await?;
        }

        let keysets = self.get_mint_keysets(&mint_url).await?;

        let mut restored_value = Amount::ZERO;

        for keyset in keysets {
            let keys = self.get_keyset_keys(&mint_url, keyset.id).await?;
            let mut empty_batch = 0;
            let mut start_counter = 0;

            while empty_batch.lt(&3) {
                let premint_secrets = PreMintSecrets::restore_batch(
                    keyset.id,
                    &self.mnemonic.clone().unwrap(),
                    start_counter,
                    start_counter + 100,
                )?;

                debug!(
                    "Attempting to restore counter {}-{} for mint {} keyset {}",
                    start_counter,
                    start_counter + 100,
                    mint_url,
                    keyset.id
                );

                let restore_request = RestoreRequest {
                    outputs: premint_secrets.blinded_messages(),
                };

                let response = self
                    .client
                    .post_restore(mint_url.clone().try_into()?, restore_request)
                    .await
                    .unwrap();

                if response.signatures.is_empty() {
                    empty_batch += 1;
                    start_counter += 100;
                    continue;
                }

                let premint_secrets: Vec<_> = premint_secrets
                    .secrets
                    .iter()
                    .filter(|p| response.outputs.contains(&p.blinded_message))
                    .collect();

                let premint_secrets: Vec<_> = premint_secrets
                    .iter()
                    .filter(|p| response.outputs.contains(&p.blinded_message))
                    .collect();

                // the response outputs and premint secrets should be the same after filtering
                // blinded messages the mint did not have signatures for
                assert_eq!(response.outputs.len(), premint_secrets.len());

                let proofs = construct_proofs(
                    response.signatures,
                    premint_secrets.iter().map(|p| p.r.clone()).collect(),
                    premint_secrets.iter().map(|p| p.secret.clone()).collect(),
                    &keys,
                )?;

                debug!("Restored {} proofs", proofs.len());

                self.localstore
                    .increment_keyset_counter(&keyset.id, proofs.len() as u64)
                    .await?;

                let states = self
                    .check_proofs_spent(mint_url.clone(), proofs.clone())
                    .await?;

                let unspent_proofs: Vec<Proof> = proofs
                    .iter()
                    .zip(states)
                    .filter(|(_, state)| !state.state.eq(&State::Spent))
                    .map(|(p, _)| p)
                    .cloned()
                    .collect();

                restored_value += unspent_proofs.iter().map(|p| p.amount).sum();

                self.localstore
                    .add_proofs(mint_url.clone(), unspent_proofs)
                    .await?;

                empty_batch = 0;
                start_counter += 100;
            }
        }
        Ok(restored_value)
    }
}

/*
#[cfg(test)]
mod tests {

    use std::collections::{HashMap, HashSet};

    use super::*;

    use crate::client::Client;
    use crate::mint::Mint;
    use cashu::nuts::nut04;

    #[test]
    fn test_wallet() {
        let mut mint = Mint::new(
            "supersecretsecret",
            "0/0/0/0",
            HashMap::new(),
            HashSet::new(),
            32,
        );

        let keys = mint.active_keyset_pubkeys();

        let client = Client::new("https://cashu-rs.thesimplekid.space/").unwrap();

        let wallet = Wallet::new(client, keys.keys);

        let blinded_messages = BlindedMessages::random(Amount::from_sat(64)).unwrap();

        let mint_request = nut04::MintRequest {
            outputs: blinded_messages.blinded_messages.clone(),
        };

        let res = mint.process_mint_request(mint_request).unwrap();

        let proofs = wallet
            .process_split_response(blinded_messages, res.promises)
            .unwrap();
        for proof in &proofs {
            mint.verify_proof(proof).unwrap();
        }

        let split = wallet.create_split(proofs.clone()).unwrap();

        let split_request = split.split_payload;

        let split_response = mint.process_split_request(split_request).unwrap();
        let p = split_response.promises;

        let snd_proofs = wallet
            .process_split_response(split.blinded_messages, p.unwrap())
            .unwrap();

        let mut error = false;
        for proof in &snd_proofs {
            if let Err(err) = mint.verify_proof(proof) {
                println!("{err}{:?}", serde_json::to_string(proof));
                error = true;
            }
        }

        if error {
            panic!()
        }
    }
}
*/
