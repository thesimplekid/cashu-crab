//! SQLite Storage for CDK

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use cdk::cdk_database;
use cdk::cdk_database::MintDatabase;
use cdk::dhke::hash_to_curve;
use cdk::mint::MintKeySetInfo;
use cdk::nuts::{
    BlindSignature, CurrencyUnit, Id, MeltQuoteState, MintQuoteState, Proof, PublicKey,
};
use cdk::secret::Secret;
use cdk::types::{MeltQuote, MintQuote};
use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::Mutex;

use super::error::Error;
use crate::migrations::migrate_00_to_01;

const ACTIVE_KEYSETS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("active_keysets");
const KEYSETS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("keysets");
const MINT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("mint_quotes");
const MELT_QUOTES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("melt_quotes");
const PENDING_PROOFS_TABLE: TableDefinition<[u8; 33], &str> =
    TableDefinition::new("pending_proofs");
const SPENT_PROOFS_TABLE: TableDefinition<[u8; 33], &str> = TableDefinition::new("spent_proofs");
const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
// Key is hex blinded_message B_ value is blinded_signature
const BLINDED_SIGNATURES: TableDefinition<[u8; 33], &str> =
    TableDefinition::new("blinded_signatures");

const DATABASE_VERSION: u32 = 1;

/// Mint Redbdatabase
#[derive(Debug, Clone)]
pub struct MintRedbDatabase {
    db: Arc<Mutex<Database>>,
}

impl MintRedbDatabase {
    /// Create new [`MintRedbDatabase`]
    pub fn new(path: &Path) -> Result<Self, Error> {
        {
            // Check database version

            let db = Arc::new(Database::create(path)?);

            // Check database version
            let read_txn = db.begin_read()?;
            let table = read_txn.open_table(CONFIG_TABLE);

            let db_version = match table {
                Ok(table) => table.get("db_version")?.map(|v| v.value().to_owned()),
                Err(_) => None,
            };
            match db_version {
                Some(db_version) => {
                    let mut current_file_version = u32::from_str(&db_version)?;
                    match current_file_version.cmp(&DATABASE_VERSION) {
                        Ordering::Less => {
                            tracing::info!(
                                "Database needs to be upgraded at {} current is {}",
                                current_file_version,
                                DATABASE_VERSION
                            );
                            if current_file_version == 0 {
                                current_file_version = migrate_00_to_01(Arc::clone(&db))?;
                            }

                            if current_file_version != DATABASE_VERSION {
                                tracing::warn!(
                                    "Database upgrade did not complete at {} current is {}",
                                    current_file_version,
                                    DATABASE_VERSION
                                );
                                return Err(Error::UnknownDatabaseVersion);
                            }
                        }
                        Ordering::Equal => {
                            tracing::info!("Database is at current version {}", DATABASE_VERSION);
                        }
                        Ordering::Greater => {
                            tracing::warn!(
                                "Database upgrade did not complete at {} current is {}",
                                current_file_version,
                                DATABASE_VERSION
                            );
                            return Err(Error::UnknownDatabaseVersion);
                        }
                    }
                }
                None => {
                    let write_txn = db.begin_write()?;
                    {
                        let mut table = write_txn.open_table(CONFIG_TABLE)?;
                        // Open all tables to init a new db
                        let _ = write_txn.open_table(ACTIVE_KEYSETS_TABLE)?;
                        let _ = write_txn.open_table(KEYSETS_TABLE)?;
                        let _ = write_txn.open_table(MINT_QUOTES_TABLE)?;
                        let _ = write_txn.open_table(MELT_QUOTES_TABLE)?;
                        let _ = write_txn.open_table(PENDING_PROOFS_TABLE)?;
                        let _ = write_txn.open_table(SPENT_PROOFS_TABLE)?;
                        let _ = write_txn.open_table(BLINDED_SIGNATURES)?;

                        table.insert("db_version", DATABASE_VERSION.to_string().as_str())?;
                    }

                    write_txn.commit()?;
                }
            }
            drop(db);
        }

        let db = Database::create(path)?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
        })
    }
}

#[async_trait]
impl MintDatabase for MintRedbDatabase {
    type Err = cdk_database::Error;

    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(ACTIVE_KEYSETS_TABLE)
                .map_err(Error::from)?;
            table
                .insert(unit.to_string().as_str(), id.to_string().as_str())
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(ACTIVE_KEYSETS_TABLE)
            .map_err(Error::from)?;

        if let Some(id) = table.get(unit.to_string().as_str()).map_err(Error::from)? {
            return Ok(Some(Id::from_str(id.value()).map_err(Error::from)?));
        }

        Ok(None)
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(ACTIVE_KEYSETS_TABLE)
            .map_err(Error::from)?;

        let mut active_keysets = HashMap::new();

        for (unit, id) in (table.iter().map_err(Error::from)?).flatten() {
            let unit = CurrencyUnit::from(unit.value());
            let id = Id::from_str(id.value()).map_err(Error::from)?;

            active_keysets.insert(unit, id);
        }

        Ok(active_keysets)
    }

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;
            table
                .insert(
                    keyset.id.to_string().as_str(),
                    serde_json::to_string(&keyset)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_keyset_info(&self, keyset_id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

        match table
            .get(keyset_id.to_string().as_str())
            .map_err(Error::from)?
        {
            Some(keyset) => Ok(serde_json::from_str(keyset.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn.open_table(KEYSETS_TABLE).map_err(Error::from)?;

        let mut keysets = Vec::new();

        for (_id, keyset) in (table.iter().map_err(Error::from)?).flatten() {
            let keyset = serde_json::from_str(keyset.value()).map_err(Error::from)?;

            keysets.push(keyset)
        }

        Ok(keysets)
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    quote.id.as_str(),
                    serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        match table.get(quote_id).map_err(Error::from)? {
            Some(quote) => Ok(serde_json::from_str(quote.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn update_mint_quote_state(
        &self,
        quote_id: &str,
        state: MintQuoteState,
    ) -> Result<MintQuoteState, Self::Err> {
        let db = self.db.lock().await;

        let mut mint_quote: MintQuote;
        {
            let read_txn = db.begin_read().map_err(Error::from)?;
            let table = read_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;

            let quote_guard = table
                .get(quote_id)
                .map_err(Error::from)?
                .ok_or(Error::UnknownMintInfo)?;

            let quote = quote_guard.value();

            mint_quote = serde_json::from_str(quote).map_err(Error::from)?;
        }

        let current_state = mint_quote.state;
        mint_quote.state = state;

        let write_txn = db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;

            table
                .insert(
                    quote_id,
                    serde_json::to_string(&mint_quote)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

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
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MINT_QUOTES_TABLE)
            .map_err(Error::from)?;

        let mut quotes = Vec::new();

        for (_id, quote) in (table.iter().map_err(Error::from)?).flatten() {
            let quote = serde_json::from_str(quote.value()).map_err(Error::from)?;

            quotes.push(quote)
        }

        Ok(quotes)
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MINT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table.remove(quote_id).map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table
                .insert(
                    quote.id.as_str(),
                    serde_json::to_string(&quote).map_err(Error::from)?.as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MELT_QUOTES_TABLE)
            .map_err(Error::from)?;

        let quote = table.get(quote_id).map_err(Error::from)?;

        Ok(quote.map(|q| serde_json::from_str(q.value()).unwrap()))
    }

    async fn update_melt_quote_state(
        &self,
        quote_id: &str,
        state: MeltQuoteState,
    ) -> Result<MeltQuoteState, Self::Err> {
        let db = self.db.lock().await;
        let mut melt_quote: MeltQuote;
        {
            let read_txn = db.begin_read().map_err(Error::from)?;
            let table = read_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;

            let quote_guard = table
                .get(quote_id)
                .map_err(Error::from)?
                .ok_or(Error::UnknownMintInfo)?;

            let quote = quote_guard.value();

            melt_quote = serde_json::from_str(quote).map_err(Error::from)?;
        }

        let current_state = melt_quote.state;
        melt_quote.state = state;

        let write_txn = db.begin_write().map_err(Error::from)?;
        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;

            table
                .insert(
                    quote_id,
                    serde_json::to_string(&melt_quote)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(current_state)
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(MELT_QUOTES_TABLE)
            .map_err(Error::from)?;

        let mut quotes = Vec::new();

        for (_id, quote) in (table.iter().map_err(Error::from)?).flatten() {
            let quote = serde_json::from_str(quote.value()).map_err(Error::from)?;

            quotes.push(quote)
        }

        Ok(quotes)
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(MELT_QUOTES_TABLE)
                .map_err(Error::from)?;
            table.remove(quote_id).map_err(Error::from)?;
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_spent_proofs(&self, proofs: Vec<Proof>) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(SPENT_PROOFS_TABLE)
                .map_err(Error::from)?;
            for proof in proofs {
                let y: PublicKey = hash_to_curve(&proof.secret.to_bytes()).map_err(Error::from)?;
                table
                    .insert(
                        y.to_bytes(),
                        serde_json::to_string(&proof).map_err(Error::from)?.as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_spent_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(SPENT_PROOFS_TABLE)
            .map_err(Error::from)?;

        match table.get(y.to_bytes()).map_err(Error::from)? {
            Some(proof) => Ok(serde_json::from_str(proof.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(SPENT_PROOFS_TABLE)
            .map_err(Error::from)?;

        let y: PublicKey = hash_to_curve(&secret.to_bytes())?;

        match table.get(y.to_bytes()).map_err(Error::from)? {
            Some(proof) => Ok(serde_json::from_str(proof.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn add_pending_proofs(&self, proofs: Vec<Proof>) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(PENDING_PROOFS_TABLE)
                .map_err(Error::from)?;
            for proof in proofs {
                table
                    .insert(
                        hash_to_curve(&proof.secret.to_bytes())?.to_bytes(),
                        serde_json::to_string(&proof).map_err(Error::from)?.as_str(),
                    )
                    .map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_pending_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(PENDING_PROOFS_TABLE)
            .map_err(Error::from)?;

        match table.get(y.to_bytes()).map_err(Error::from)? {
            Some(proof) => Ok(serde_json::from_str(proof.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn get_pending_proof_by_secret(
        &self,
        secret: &Secret,
    ) -> Result<Option<Proof>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(PENDING_PROOFS_TABLE)
            .map_err(Error::from)?;

        let secret_hash = hash_to_curve(&secret.to_bytes())?;

        match table.get(secret_hash.to_bytes()).map_err(Error::from)? {
            Some(proof) => Ok(serde_json::from_str(proof.value()).map_err(Error::from)?),
            None => Ok(None),
        }
    }

    async fn remove_pending_proofs(&self, secrets: Vec<&Secret>) -> Result<(), Self::Err> {
        let db = self.db.lock().await;

        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(PENDING_PROOFS_TABLE)
                .map_err(Error::from)?;
            for secret in secrets {
                let secret_hash = hash_to_curve(&secret.to_bytes()).map_err(Error::from)?;
                table.remove(secret_hash.to_bytes()).map_err(Error::from)?;
            }
        }
        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn add_blinded_signature(
        &self,
        blinded_message: PublicKey,
        blinded_signature: BlindSignature,
    ) -> Result<(), Self::Err> {
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(Error::from)?;

        {
            let mut table = write_txn
                .open_table(BLINDED_SIGNATURES)
                .map_err(Error::from)?;
            table
                .insert(
                    blinded_message.to_bytes(),
                    serde_json::to_string(&blinded_signature)
                        .map_err(Error::from)?
                        .as_str(),
                )
                .map_err(Error::from)?;
        }

        write_txn.commit().map_err(Error::from)?;

        Ok(())
    }

    async fn get_blinded_signature(
        &self,
        blinded_message: &PublicKey,
    ) -> Result<Option<BlindSignature>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(BLINDED_SIGNATURES)
            .map_err(Error::from)?;

        match table.get(blinded_message.to_bytes()).map_err(Error::from)? {
            Some(blind_signature) => {
                Ok(serde_json::from_str(blind_signature.value()).map_err(Error::from)?)
            }
            None => Ok(None),
        }
    }

    async fn get_blinded_signatures(
        &self,
        blinded_messages: Vec<PublicKey>,
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(Error::from)?;
        let table = read_txn
            .open_table(BLINDED_SIGNATURES)
            .map_err(Error::from)?;

        let mut signatures = Vec::with_capacity(blinded_messages.len());

        for blinded_message in blinded_messages {
            match table.get(blinded_message.to_bytes()).map_err(Error::from)? {
                Some(blind_signature) => signatures.push(Some(
                    serde_json::from_str(blind_signature.value()).map_err(Error::from)?,
                )),
                None => signatures.push(None),
            }
        }

        Ok(signatures)
    }
}
