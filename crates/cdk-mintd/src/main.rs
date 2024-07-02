//! CDK Mint Server

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use bip39::Mnemonic;
use cdk::cdk_database::{self, MintDatabase};
use cdk::cdk_lightning::MintLightning;
use cdk::mint::Mint;
use cdk::nuts::MintInfo;
use cdk::{cdk_lightning, Amount};
use cdk_cln::Cln;
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use clap::Parser;
use cli::CLIArgs;
use config::{DatabaseEngine, LnBackend};
use futures::StreamExt;
use lightning_invoice::Bolt11Invoice;

mod cli;
mod config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let args = CLIArgs::parse();

    // get config file name from args
    let config_file_arg = match args.config {
        Some(c) => c,
        None => work_dir()?.join("config.toml"),
    };

    let settings = config::Settings::new(&Some(config_file_arg));

    let db_path = match args.db {
        Some(path) => path,
        None => settings.info.clone().db_path,
    };

    let localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync> =
        match settings.database.engine {
            DatabaseEngine::Sqlite => {
                let sqlite_db = MintSqliteDatabase::new(&db_path).await?;

                sqlite_db.migrate().await;

                Arc::new(sqlite_db)
            }
            DatabaseEngine::Redb => Arc::new(MintRedbDatabase::new(&db_path)?),
        };

    let mint_info = MintInfo::default();

    let mnemonic = Mnemonic::from_str(&settings.info.mnemonic)?;

    let mint = Mint::new(
        &settings.info.url,
        &mnemonic.to_seed_normalized(""),
        mint_info,
        localstore,
        Amount::ZERO,
        0.0,
    )
    .await?;

    let ln: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync> =
        match settings.ln.ln_backend {
            LnBackend::Cln => {
                let cln_socket = expand_path(
                    settings
                        .ln
                        .cln_path
                        .clone()
                        .ok_or(anyhow!("cln socket not defined"))?
                        .to_str()
                        .ok_or(anyhow!("cln socket not defined"))?,
                )
                .ok_or(anyhow!("cln socket not defined"))?;

                Arc::new(Cln::new(cln_socket).await?)
            }
        };

    let mint = Arc::new(mint);

    // Check the status of any mint quotes that are pending
    // In the event that the mint server is down but the ln node is not
    // it is possible that a mint quote was paid but the mint has not been updated
    // this will check and update the mint state of those quotes
    check_pending_quotes(Arc::clone(&mint), Arc::clone(&ln)).await?;

    let mint_clone = Arc::clone(&mint);
    let ln_clone = ln.clone();

    tokio::spawn(async move {
        loop {
            let mut stream = ln_clone.wait_any_invoice().await.unwrap();

            while let Some(invoice) = stream.next().await {
                if let Err(err) =
                    handle_paid_invoice(mint_clone.clone(), &invoice.to_string()).await
                {
                    tracing::warn!("{:?}", err);
                }
            }
        }
    });

    let mint_url = settings.info.url;
    let listen_addr = settings.info.listen_host;
    let listen_port = settings.info.listen_port;

    cdk_axum::start_server(&mint_url, &listen_addr, listen_port, mint, ln).await?;

    Ok(())
}

async fn handle_paid_invoice(mint: Arc<Mint>, request: &str) -> Result<()> {
    if let Ok(Some(mint_quote)) = mint.localstore.get_mint_quote_by_request(request).await {
        mint.localstore
            .update_mint_quote_state(&mint_quote.id, cdk::nuts::MintQuoteState::Paid)
            .await?;
    }
    Ok(())
}

async fn check_pending_quotes(
    mint: Arc<Mint>,
    ln: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
) -> Result<()> {
    let pending_quotes = mint.get_pending_mint_quotes().await?;

    for quote in pending_quotes {
        let payment_hash = Bolt11Invoice::from_str(&quote.request)?
            .payment_hash()
            .to_string();
        let state = ln.check_invoice_status(&payment_hash).await?;

        if state != quote.state {
            mint.localstore
                .update_mint_quote_state(&quote.id, state)
                .await?;
        }
    }

    Ok(())
}

fn expand_path(path: &str) -> Option<PathBuf> {
    if path.starts_with('~') {
        if let Some(home_dir) = home::home_dir().as_mut() {
            let remainder = &path[2..];
            home_dir.push(remainder);
            let expanded_path = home_dir;
            Some(expanded_path.clone())
        } else {
            None
        }
    } else {
        Some(PathBuf::from(path))
    }
}

fn work_dir() -> Result<PathBuf> {
    let home_dir = home::home_dir().ok_or(anyhow!("Unknown home dir"))?;

    Ok(home_dir.join(".cdk-mintd"))
}
