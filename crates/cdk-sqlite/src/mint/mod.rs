//! SQLite Mint

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use bitcoin::bip32::DerivationPath;
use cdk::cdk_database::{self, MintDatabase};
use cdk::mint::MintKeySetInfo;
use cdk::nuts::nut05::QuoteState;
use cdk::nuts::{
    BlindSignature, CurrencyUnit, Id, MeltQuoteState, MintQuoteState, Proof, Proofs, PublicKey,
};
use cdk::secret::Secret;
use cdk::types::{MeltQuote, MintQuote};
use cdk::Amount;
use error::Error;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqliteRow};
use sqlx::{ConnectOptions, Row};

pub mod error;

/// Mint SQLite Database
#[derive(Debug, Clone)]
pub struct MintSqliteDatabase {
    pool: SqlitePool,
}

impl MintSqliteDatabase {
    /// Create new [`MintSqliteDatabase`]
    pub async fn new(path: &Path) -> Result<Self, Error> {
        let path = path.to_str().ok_or(Error::InvalidDbPath)?;
        let _conn = SqliteConnectOptions::from_str(path)?
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .read_only(false)
            .create_if_missing(true)
            .auto_vacuum(sqlx::sqlite::SqliteAutoVacuum::Full)
            .connect()
            .await?;

        let pool = SqlitePool::connect(path).await?;

        Ok(Self { pool })
    }

    /// Migrate [`MintSqliteDatabase`]
    pub async fn migrate(&self) {
        sqlx::migrate!("./src/mint/migrations")
            .run(&self.pool)
            .await
            .expect("Could not run migrations");
    }
}

#[async_trait]
impl MintDatabase for MintSqliteDatabase {
    type Err = cdk_database::Error;

    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
UPDATE keyset
SET active=TRUE
WHERE unit IS ?
AND id IS ?;
        "#,
        )
        .bind(unit.to_string())
        .bind(id.to_string())
        .execute(&self.pool)
        .await
        // TODO: should check if error is not found and return none
        .map_err(Error::from)?;

        Ok(())
    }
    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT id
FROM keyset
WHERE active = 1
AND unit IS ?
        "#,
        )
        .bind(unit.to_string())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(
            Id::from_str(rec.try_get("id").map_err(Error::from)?).map_err(Error::from)?,
        ))
    }
    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        let recs = sqlx::query(
            r#"
SELECT id, unit
FROM keyset
WHERE active = 1
        "#,
        )
        .fetch_all(&self.pool)
        .await
        // TODO: should check if error is not found and return none
        .map_err(Error::from)?;

        let keysets = recs
            .iter()
            .filter_map(|r| match Id::from_str(r.get("id")) {
                Ok(id) => Some((CurrencyUnit::from(r.get::<'_, &str, &str>("unit")), id)),
                Err(_) => None,
            })
            .collect();

        Ok(keysets)
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
INSERT OR REPLACE INTO mint_quote
(id, mint_url, amount, unit, request, state, expiry)
VALUES (?, ?, ?, ?, ?, ?, ?);
        "#,
        )
        .bind(quote.id.to_string())
        .bind(quote.mint_url.to_string())
        .bind(u64::from(quote.amount) as i64)
        .bind(quote.unit.to_string())
        .bind(quote.request)
        .bind(quote.state.to_string())
        .bind(quote.expiry as i64)
        .execute(&self.pool)
        .await
        // TODO: should check if error is not found and return none
        .map_err(Error::from)?;

        Ok(())
    }
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id)
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_mint_quote(rec)?))
    }

    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
WHERE request=?;
        "#,
        )
        .bind(request)
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_mint_quote(rec)?))
    }
    async fn update_mint_quote_state(
        &self,
        quote_id: &str,
        state: MintQuoteState,
    ) -> Result<MintQuoteState, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id)
        .fetch_one(&mut transaction)
        .await
        .map_err(Error::from)?;

        let quote = sqlite_row_to_mint_quote(rec)?;

        sqlx::query(
            r#"
        UPDATE mint_quote SET state = ? WHERE id = ?
        "#,
        )
        .bind(state.to_string())
        .bind(quote_id)
        .execute(&mut transaction)
        .await
        .map_err(Error::from)?;

        transaction.commit().await.map_err(Error::from)?;

        Ok(quote.state)
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM mint_quote
        "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)?;

        let mint_quotes = rec.into_iter().flat_map(sqlite_row_to_mint_quote).collect();

        Ok(mint_quotes)
    }
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
DELETE FROM mint_quote
WHERE id=?
        "#,
        )
        .bind(quote_id)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
INSERT OR REPLACE INTO melt_quote
(id, unit, amount, request, fee_reserve, state, expiry, payment_preimage)
VALUES (?, ?, ?, ?, ?, ?, ?, ?);
        "#,
        )
        .bind(quote.id.to_string())
        .bind(quote.unit.to_string())
        .bind(u64::from(quote.amount) as i64)
        .bind(quote.request)
        .bind(u64::from(quote.fee_reserve) as i64)
        .bind(quote.state.to_string())
        .bind(quote.expiry as i64)
        .bind(quote.payment_preimage)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM melt_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id)
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_melt_quote(rec)?))
    }
    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM melt_quote
        "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)?;

        let melt_quotes = rec.into_iter().flat_map(sqlite_row_to_melt_quote).collect();

        Ok(melt_quotes)
    }

    async fn update_melt_quote_state(
        &self,
        quote_id: &str,
        state: MeltQuoteState,
    ) -> Result<MeltQuoteState, Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        let rec = sqlx::query(
            r#"
SELECT *
FROM melt_quote
WHERE id=?;
        "#,
        )
        .bind(quote_id)
        .fetch_one(&mut transaction)
        .await
        .map_err(Error::from)?;

        let quote = sqlite_row_to_melt_quote(rec)?;

        sqlx::query(
            r#"
        UPDATE melt_quote SET state = ? WHERE id = ?
        "#,
        )
        .bind(state.to_string())
        .bind(quote_id)
        .execute(&mut transaction)
        .await
        .map_err(Error::from)?;

        transaction.commit().await.map_err(Error::from)?;

        Ok(quote.state)
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
DELETE FROM melt_quote
WHERE id=?
        "#,
        )
        .bind(quote_id)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
INSERT INTO keyset
(id, unit, active, valid_from, valid_to, derivation_path, max_order)
VALUES (?, ?, ?, ?, ?, ?, ?);
        "#,
        )
        .bind(keyset.id.to_string())
        .bind(keyset.unit.to_string())
        .bind(keyset.active)
        .bind(keyset.valid_from as i64)
        .bind(keyset.valid_to.map(|v| v as i64))
        .bind(keyset.derivation_path.to_string())
        .bind(keyset.max_order)
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }
    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM keyset
WHERE id=?;
        "#,
        )
        .bind(id.to_string())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_keyset_info(rec)?))
    }
    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
        let recs = sqlx::query(
            r#"
SELECT *
FROM keyset;
        "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(recs
            .into_iter()
            .flat_map(sqlite_row_to_keyset_info)
            .collect())
    }

    async fn add_spent_proofs(&self, proofs: Proofs) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;

        for proof in proofs {
            sqlx::query(
                r#"
INSERT OR REPLACE INTO proof
(y, amount, keyset_id, secret, c, witness, state)
VALUES (?, ?, ?, ?, ?, ?, ?);
        "#,
            )
            .bind(proof.y()?.to_bytes().to_vec())
            .bind(u64::from(proof.amount) as i64)
            .bind(proof.keyset_id.to_string())
            .bind(proof.secret.to_string())
            .bind(proof.c.to_bytes().to_vec())
            .bind(proof.witness.map(|w| serde_json::to_string(&w).unwrap()))
            .bind("SPENT")
            .execute(&mut transaction)
            .await
            .map_err(Error::from)?;
        }
        transaction.commit().await.map_err(Error::from)?;
        Ok(())
    }
    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM proof
WHERE secret=?
AND state="SPENT";
        "#,
        )
        .bind(secret.to_string())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_proof(rec)?))
    }
    async fn get_spent_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM proof
WHERE y=?
AND state="SPENT";
        "#,
        )
        .bind(y.to_bytes().to_vec())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_proof(rec)?))
    }

    async fn add_pending_proofs(&self, proofs: Proofs) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        for proof in proofs {
            sqlx::query(
                r#"
INSERT OR REPLACE INTO proof
(y, amount, keyset_id, secret, c, witness, state)
VALUES (?, ?, ?, ?, ?, ?, ?);
        "#,
            )
            .bind(proof.y()?.to_bytes().to_vec())
            .bind(u64::from(proof.amount) as i64)
            .bind(proof.keyset_id.to_string())
            .bind(proof.secret.to_string())
            .bind(proof.c.to_bytes().to_vec())
            .bind(proof.witness.map(|w| serde_json::to_string(&w).unwrap()))
            .bind("PENDING")
            .execute(&mut transaction)
            .await
            .map_err(Error::from)?;
        }
        transaction.commit().await.map_err(Error::from)?;

        Ok(())
    }
    async fn get_pending_proof_by_secret(
        &self,
        secret: &Secret,
    ) -> Result<Option<Proof>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM proof
WHERE secret=?
AND state="PENDING";
        "#,
        )
        .bind(secret.to_string())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_proof(rec)?))
    }
    async fn get_pending_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM proof
WHERE y=?
AND state="PENDING";
        "#,
        )
        .bind(y.to_bytes().to_vec())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };
        Ok(Some(sqlite_row_to_proof(rec)?))
    }
    async fn remove_pending_proofs(&self, secrets: Vec<&Secret>) -> Result<(), Self::Err> {
        let mut transaction = self.pool.begin().await.map_err(Error::from)?;
        for secret in secrets {
            sqlx::query(
                r#"
DELETE FROM proof
WHERE secret=?
AND state="PENDING";
        "#,
            )
            .bind(secret.to_string())
            .execute(&mut transaction)
            .await
            .map_err(Error::from)?;
        }
        transaction.commit().await.map_err(Error::from)?;

        Ok(())
    }

    async fn add_blinded_signature(
        &self,
        blinded_message: PublicKey,
        blinded_signature: BlindSignature,
    ) -> Result<(), Self::Err> {
        sqlx::query(
            r#"
INSERT INTO blind_signature
(y, amount, keyset_id, c)
VALUES (?, ?, ?, ?);
        "#,
        )
        .bind(blinded_message.to_bytes().to_vec())
        .bind(u64::from(blinded_signature.amount) as i64)
        .bind(blinded_signature.keyset_id.to_string())
        .bind(blinded_signature.c.to_bytes().to_vec())
        .execute(&self.pool)
        .await
        .map_err(Error::from)?;

        Ok(())
    }
    async fn get_blinded_signature(
        &self,
        blinded_message: &PublicKey,
    ) -> Result<Option<BlindSignature>, Self::Err> {
        let rec = sqlx::query(
            r#"
SELECT *
FROM blind_signature
WHERE y=?;
        "#,
        )
        .bind(blinded_message.to_bytes().to_vec())
        .fetch_one(&self.pool)
        .await;

        let rec = match rec {
            Ok(rec) => rec,
            Err(err) => match err {
                sqlx::Error::RowNotFound => return Ok(None),
                _ => return Err(Error::SQLX(err).into()),
            },
        };

        Ok(Some(sqlite_row_to_blind_signature(rec)?))
    }
    async fn get_blinded_signatures(
        &self,
        blinded_messages: Vec<PublicKey>,
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let mut signatures = Vec::with_capacity(blinded_messages.len());
        for message in blinded_messages {
            let rec = sqlx::query(
                r#"
SELECT *
FROM blind_signature
WHERE y=?;
        "#,
            )
            .bind(message.to_bytes().to_vec())
            .fetch_one(&self.pool)
            .await;

            if let Ok(row) = rec {
                let blinded = sqlite_row_to_blind_signature(row)?;

                signatures.push(Some(blinded));
            } else {
                signatures.push(None);
            }
        }

        Ok(signatures)
    }
}

fn sqlite_row_to_keyset_info(row: SqliteRow) -> Result<MintKeySetInfo, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_active: bool = row.try_get("active").map_err(Error::from)?;
    let row_valid_from: i64 = row.try_get("valid_from").map_err(Error::from)?;
    let row_valid_to: Option<i64> = row.try_get("valid_to").map_err(Error::from)?;
    let row_derivation_path: String = row.try_get("derivation_path").map_err(Error::from)?;
    let row_max_order: u8 = row.try_get("max_order").map_err(Error::from)?;

    Ok(MintKeySetInfo {
        id: Id::from_str(&row_id).map_err(Error::from)?,
        unit: CurrencyUnit::from(&row_unit),
        active: row_active,
        valid_from: row_valid_from as u64,
        valid_to: row_valid_to.map(|v| v as u64),
        derivation_path: DerivationPath::from_str(&row_derivation_path).map_err(Error::from)?,
        max_order: row_max_order,
    })
}

fn sqlite_row_to_mint_quote(row: SqliteRow) -> Result<MintQuote, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_mint_url: String = row.try_get("mint_url").map_err(Error::from)?;
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_request: String = row.try_get("request").map_err(Error::from)?;
    let row_state: String = row.try_get("state").map_err(Error::from)?;
    let row_expiry: i64 = row.try_get("expiry").map_err(Error::from)?;

    Ok(MintQuote {
        id: row_id,
        mint_url: row_mint_url.into(),
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from(row_unit),
        request: row_request,
        state: MintQuoteState::from_str(&row_state).map_err(Error::from)?,
        expiry: row_expiry as u64,
    })
}

fn sqlite_row_to_melt_quote(row: SqliteRow) -> Result<MeltQuote, Error> {
    let row_id: String = row.try_get("id").map_err(Error::from)?;
    let row_unit: String = row.try_get("unit").map_err(Error::from)?;
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let row_request: String = row.try_get("request").map_err(Error::from)?;
    let row_fee_reserve: i64 = row.try_get("fee_reserve").map_err(Error::from)?;
    let row_state: String = row.try_get("state").map_err(Error::from)?;
    let row_expiry: i64 = row.try_get("expiry").map_err(Error::from)?;
    let row_preimage: Option<String> = row.try_get("payment_preimage").map_err(Error::from)?;

    Ok(MeltQuote {
        id: row_id,
        amount: Amount::from(row_amount as u64),
        unit: CurrencyUnit::from(row_unit),
        request: row_request,
        fee_reserve: Amount::from(row_fee_reserve as u64),
        state: QuoteState::from_str(&row_state)?,
        expiry: row_expiry as u64,
        payment_preimage: row_preimage,
    })
}

fn sqlite_row_to_proof(row: SqliteRow) -> Result<Proof, Error> {
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let keyset_id: String = row.try_get("keyset_id").map_err(Error::from)?;
    let row_secret: String = row.try_get("secret").map_err(Error::from)?;
    let row_c: Vec<u8> = row.try_get("c").map_err(Error::from)?;
    let row_witness: Option<String> = row.try_get("witness").map_err(Error::from)?;

    Ok(Proof {
        amount: Amount::from(row_amount as u64),
        keyset_id: Id::from_str(&keyset_id)?,
        secret: Secret::from_str(&row_secret)?,
        c: PublicKey::from_slice(&row_c)?,
        witness: row_witness.and_then(|w| serde_json::from_str(&w).ok()),
        dleq: None,
    })
}

fn sqlite_row_to_blind_signature(row: SqliteRow) -> Result<BlindSignature, Error> {
    let row_amount: i64 = row.try_get("amount").map_err(Error::from)?;
    let keyset_id: String = row.try_get("keyset_id").map_err(Error::from)?;
    let row_c: Vec<u8> = row.try_get("c").map_err(Error::from)?;

    Ok(BlindSignature {
        amount: Amount::from(row_amount as u64),
        keyset_id: Id::from_str(&keyset_id)?,
        c: PublicKey::from_slice(&row_c)?,
        dleq: None,
    })
}
