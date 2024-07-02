//! CDK lightning backend for CLN

use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdk::cdk_lightning::{self, MintLightning, PayInvoiceResponse};
use cdk::nuts::{MeltQuoteState, MintQuoteState};
use cdk::util::{hex, unix_time};
use cdk::Bolt11Invoice;
use cln_rpc::model::requests::{
    InvoiceRequest, ListinvoicesRequest, PayRequest, WaitanyinvoiceRequest,
};
use cln_rpc::model::responses::{ListinvoicesInvoicesStatus, PayStatus, WaitanyinvoiceResponse};
use cln_rpc::model::Request;
use cln_rpc::primitives::{Amount as CLN_Amount, AmountOrAny};
use error::Error;
use futures::{Stream, StreamExt};
use tokio::sync::Mutex;
use uuid::Uuid;

pub mod error;

#[derive(Clone)]
pub struct Cln {
    rpc_socket: PathBuf,
    cln_client: Arc<Mutex<cln_rpc::ClnRpc>>,
}

impl Cln {
    pub async fn new(rpc_socket: PathBuf) -> Result<Self, Error> {
        let cln_client = cln_rpc::ClnRpc::new(&rpc_socket).await?;

        Ok(Self {
            rpc_socket,
            cln_client: Arc::new(Mutex::new(cln_client)),
        })
    }
}

#[async_trait]
impl MintLightning for Cln {
    type Err = cdk_lightning::Error;

    async fn wait_any_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Bolt11Invoice> + Send>>, Self::Err> {
        let last_pay_index = self.get_last_pay_index().await?;
        let cln_client = cln_rpc::ClnRpc::new(&self.rpc_socket).await?;

        Ok(futures::stream::unfold(
            (cln_client, last_pay_index),
            |(mut cln_client, mut last_pay_idx)| async move {
                loop {
                    let invoice_res = cln_client
                        .call(cln_rpc::Request::WaitAnyInvoice(WaitanyinvoiceRequest {
                            timeout: None,
                            lastpay_index: last_pay_idx,
                        }))
                        .await;

                    let invoice: WaitanyinvoiceResponse = match invoice_res {
                        Ok(invoice) => invoice,
                        Err(e) => {
                            tracing::warn!("Error fetching invoice: {e}");
                            // Let's not spam CLN with requests on failure
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            // Retry same request
                            continue;
                        }
                    }
                    .try_into()
                    .expect("Wrong response from CLN");

                    last_pay_idx = invoice.pay_index;

                    if let Some(bolt11) = invoice.bolt11 {
                        if let Ok(invoice) = Bolt11Invoice::from_str(&bolt11) {
                            break Some((invoice, (cln_client, last_pay_idx)));
                        }
                    }
                }
            },
        )
        .boxed())
    }

    async fn pay_invoice(
        &self,
        bolt11: Bolt11Invoice,
        partial_msats: Option<u64>,
        max_fee_msats: Option<u64>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let mut cln_client = self.cln_client.lock().await;
        let cln_response = cln_client
            .call(Request::Pay(PayRequest {
                bolt11: bolt11.to_string(),
                amount_msat: None,
                label: None,
                riskfactor: None,
                maxfeepercent: None,
                retry_for: None,
                maxdelay: None,
                exemptfee: None,
                localinvreqid: None,
                exclude: None,
                maxfee: max_fee_msats.map(CLN_Amount::from_msat),
                description: None,
                partial_msat: partial_msats.map(CLN_Amount::from_msat),
            }))
            .await
            .map_err(Error::from)?;

        let response = match cln_response {
            cln_rpc::Response::Pay(pay_response) => {
                let status = match pay_response.status {
                    PayStatus::COMPLETE => MeltQuoteState::Paid,
                    PayStatus::PENDING => MeltQuoteState::Pending,
                    PayStatus::FAILED => MeltQuoteState::Unpaid,
                };
                PayInvoiceResponse {
                    payment_preimage: Some(hex::encode(pay_response.payment_preimage.to_vec())),
                    payment_hash: pay_response.payment_hash.to_string(),
                    status,
                    total_spent_msats: pay_response.amount_sent_msat.msat(),
                }
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                return Err(cdk_lightning::Error::from(Error::WrongClnResponse));
            }
        };

        Ok(response)
    }

    async fn create_invoice(
        &self,
        amount_msats: u64,
        description: String,
        unix_expiry: u64,
    ) -> Result<Bolt11Invoice, Self::Err> {
        let time_now = unix_time();
        assert!(unix_expiry > time_now);

        let mut cln_client = self.cln_client.lock().await;

        let amount_msat = AmountOrAny::Amount(CLN_Amount::from_msat(amount_msats));
        let cln_response = cln_client
            .call(cln_rpc::Request::Invoice(InvoiceRequest {
                amount_msat,
                description,
                label: Uuid::new_v4().to_string(),
                expiry: Some(unix_expiry - time_now),
                fallbacks: None,
                preimage: None,
                cltv: None,
                deschashonly: None,
                exposeprivatechannels: None,
            }))
            .await
            .map_err(Error::from)?;

        let invoice = match cln_response {
            cln_rpc::Response::Invoice(invoice_res) => {
                Bolt11Invoice::from_str(&invoice_res.bolt11)?
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                return Err(Error::WrongClnResponse.into());
            }
        };

        Ok(invoice)
    }

    async fn check_invoice_status(&self, payment_hash: &str) -> Result<MintQuoteState, Self::Err> {
        let mut cln_client = self.cln_client.lock().await;

        let cln_response = cln_client
            .call(Request::ListInvoices(ListinvoicesRequest {
                payment_hash: Some(payment_hash.to_string()),
                label: None,
                invstring: None,
                offer_id: None,
                index: None,
                limit: None,
                start: None,
            }))
            .await
            .map_err(Error::from)?;

        let status = match cln_response {
            cln_rpc::Response::ListInvoices(invoice_response) => {
                match invoice_response.invoices.first() {
                    Some(invoice_response) => {
                        cln_invoice_status_to_mint_state(invoice_response.status)
                    }
                    None => {
                        tracing::info!(
                            "Check invoice called on unknown payment_hash: {}",
                            payment_hash
                        );
                        return Err(Error::WrongClnResponse.into());
                    }
                }
            }
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                return Err(Error::Custom("CLN returned wrong response kind".to_string()).into());
            }
        };

        Ok(status)
    }
}

impl Cln {
    async fn get_last_pay_index(&self) -> Result<Option<u64>, Error> {
        let mut cln_client = self.cln_client.lock().await;
        let cln_response = cln_client
            .call(cln_rpc::Request::ListInvoices(ListinvoicesRequest {
                index: None,
                invstring: None,
                label: None,
                limit: None,
                offer_id: None,
                payment_hash: None,
                start: None,
            }))
            .await
            .map_err(Error::from)?;

        match cln_response {
            cln_rpc::Response::ListInvoices(invoice_res) => match invoice_res.invoices.last() {
                Some(last_invoice) => Ok(last_invoice.pay_index),
                None => Ok(None),
            },
            _ => {
                tracing::warn!("CLN returned wrong response kind");
                Err(Error::WrongClnResponse)
            }
        }
    }
}
pub fn fee_reserve(invoice_amount_msats: u64) -> u64 {
    (invoice_amount_msats as f64 * 0.01) as u64
}

pub fn cln_invoice_status_to_mint_state(status: ListinvoicesInvoicesStatus) -> MintQuoteState {
    match status {
        ListinvoicesInvoicesStatus::UNPAID => MintQuoteState::Unpaid,
        ListinvoicesInvoicesStatus::PAID => MintQuoteState::Paid,
        ListinvoicesInvoicesStatus::EXPIRED => MintQuoteState::Unpaid,
    }
}
