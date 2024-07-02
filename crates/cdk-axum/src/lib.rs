use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use cdk::cdk_lightning::{self, MintLightning};
use cdk::error::{Error, ErrorResponse};
use cdk::mint::Mint;
use cdk::nuts::nut05::MeltBolt11Response;
use cdk::nuts::{
    CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeysResponse, KeysetResponse,
    MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintBolt11Request,
    MintBolt11Response, MintInfo, MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteState,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use cdk::util::unix_time;
use cdk::{Amount, Bolt11Invoice};
use futures::StreamExt;

const MSATS_IN_SAT: u64 = 1000;

pub async fn create_mint_router(
    mint_url: &str,
    mint: Arc<Mint>,
    ln: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
) -> Result<Router> {
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

    let state = MintState {
        ln,
        mint,
        mint_url: mint_url.to_string(),
    };

    let mint_router = Router::new()
        .route("/keys", get(get_keys))
        .route("/keysets", get(get_keysets))
        .route("/keys/:keyset_id", get(get_keyset_pubkeys))
        .route("/swap", post(post_swap))
        .route("/mint/quote/bolt11", post(get_mint_bolt11_quote))
        .route(
            "/mint/quote/bolt11/:quote_id",
            get(get_check_mint_bolt11_quote),
        )
        .route("/mint/bolt11", post(post_mint_bolt11))
        .route("/melt/quote/bolt11", post(get_melt_bolt11_quote))
        .route(
            "/melt/quote/bolt11/:quote_id",
            get(get_check_melt_bolt11_quote),
        )
        .route("/melt/bolt11", post(post_melt_bolt11))
        .route("/checkstate", post(post_check))
        .route("/info", get(get_mint_info))
        .route("/restore", post(post_restore))
        .with_state(state);

    Ok(mint_router)
}

async fn handle_paid_invoice(mint: Arc<Mint>, request: &str) -> Result<()> {
    if let Some(quote) = mint.localstore.get_mint_quote_by_request(request).await? {
        mint.localstore
            .update_mint_quote_state(&quote.id, MintQuoteState::Paid)
            .await?;
    }

    Ok(())
}
#[derive(Clone)]
struct MintState {
    ln: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
    mint: Arc<Mint>,
    mint_url: String,
}

async fn get_keys(State(state): State<MintState>) -> Result<Json<KeysResponse>, Response> {
    let pubkeys = state.mint.pubkeys().await.map_err(into_response)?;

    Ok(Json(pubkeys))
}

async fn get_keyset_pubkeys(
    State(state): State<MintState>,
    Path(keyset_id): Path<Id>,
) -> Result<Json<KeysResponse>, Response> {
    let pubkeys = state
        .mint
        .keyset_pubkeys(&keyset_id)
        .await
        .map_err(into_response)?;

    Ok(Json(pubkeys))
}

async fn get_keysets(State(state): State<MintState>) -> Result<Json<KeysetResponse>, Response> {
    let mint = state.mint.keysets().await.map_err(into_response)?;

    Ok(Json(mint))
}

async fn get_mint_bolt11_quote(
    State(state): State<MintState>,
    Json(payload): Json<MintQuoteBolt11Request>,
) -> Result<Json<MintQuoteBolt11Response>, Response> {
    let amount = match payload.unit {
        CurrencyUnit::Sat => u64::from(payload.amount) / MSATS_IN_SAT,
        CurrencyUnit::Msat => u64::from(payload.amount),
        _ => return Err(into_response(cdk::mint::error::Error::UnsupportedUnit)),
    };

    let invoice = state
        .ln
        .create_invoice(amount, "".to_string(), unix_time() + 1800)
        .await
        .map_err(|_| into_response(Error::InvalidPaymentRequest))?;

    let quote = state
        .mint
        .new_mint_quote(
            state.mint_url.into(),
            invoice.to_string(),
            payload.unit,
            payload.amount,
            unix_time() + 1800,
        )
        .await
        .map_err(into_response)?;

    Ok(Json(quote.into()))
}

async fn get_check_mint_bolt11_quote(
    State(state): State<MintState>,
    Path(quote_id): Path<String>,
) -> Result<Json<MintQuoteBolt11Response>, Response> {
    let quote = state
        .mint
        .check_mint_quote(&quote_id)
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
}

async fn post_mint_bolt11(
    State(state): State<MintState>,
    Json(payload): Json<MintBolt11Request>,
) -> Result<Json<MintBolt11Response>, Response> {
    let res = state
        .mint
        .process_mint_request(payload)
        .await
        .map_err(into_response)?;

    Ok(Json(res))
}

async fn get_melt_bolt11_quote(
    State(state): State<MintState>,
    Json(payload): Json<MeltQuoteBolt11Request>,
) -> Result<Json<MeltQuoteBolt11Response>, Response> {
    let amount = match payload.unit {
        CurrencyUnit::Sat => Amount::from(
            payload
                .request
                .amount_milli_satoshis()
                .ok_or(Error::InvoiceAmountUndefined)
                .map_err(into_response)?
                / 1000,
        ),
        CurrencyUnit::Msat => Amount::from(
            payload
                .request
                .amount_milli_satoshis()
                .ok_or(Error::InvoiceAmountUndefined)
                .map_err(into_response)?,
        ),
        _ => return Err(into_response(cdk::mint::error::Error::UnsupportedUnit)),
    };

    let fee_reserve = Amount::from(
        (state.mint.fee_reserve.percent_fee_reserve as f64 * u64::from(amount) as f64) as u64,
    );

    let quote = state
        .mint
        .new_melt_quote(
            payload.request.to_string(),
            payload.unit,
            amount,
            fee_reserve,
            unix_time() + 1800,
        )
        .await
        .map_err(into_response)?;

    Ok(Json(quote.into()))
}

async fn get_check_melt_bolt11_quote(
    State(state): State<MintState>,
    Path(quote_id): Path<String>,
) -> Result<Json<MeltQuoteBolt11Response>, Response> {
    let quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .map_err(into_response)?;

    Ok(Json(quote))
}

async fn post_melt_bolt11(
    State(state): State<MintState>,
    Json(payload): Json<MeltBolt11Request>,
) -> Result<Json<MeltBolt11Response>, Response> {
    let quote = state
        .mint
        .verify_melt_request(&payload)
        .await
        .map_err(into_response)?;

    let invoice = Bolt11Invoice::from_str(&quote.request)
        .map_err(|_| into_response(Error::InvalidPaymentRequest))?;

    let (preimage, amount_spent) = match state
        .mint
        .localstore
        .get_mint_quote_by_request(&quote.request)
        .await
        .unwrap()
    {
        Some(mint_quote) => {
            let mut mint_quote = mint_quote;
            mint_quote.state = MintQuoteState::Paid;

            let amount = quote.amount;

            state.mint.update_mint_quote(mint_quote).await.unwrap();

            (None, amount)
        }
        None => {
            let pre = state
                .ln
                .pay_invoice(invoice, None, None)
                .await
                .map_err(|_| {
                    into_response(ErrorResponse::new(
                        cdk::error::ErrorCode::Unknown(999),
                        Some("Could not pay ln invoice".to_string()),
                        None,
                    ))
                })?;
            let amount = match quote.unit {
                CurrencyUnit::Sat => Amount::from(pre.total_spent_msats / 1000),
                CurrencyUnit::Msat => Amount::from(pre.total_spent_msats),
                _ => return Err(into_response(cdk::mint::error::Error::UnsupportedUnit)),
            };

            (pre.payment_preimage, amount)
        }
    };

    let res = state
        .mint
        .process_melt_request(&payload, preimage, amount_spent)
        .await
        .map_err(into_response)?;

    Ok(Json(res.into()))
}

async fn post_check(
    State(state): State<MintState>,
    Json(payload): Json<CheckStateRequest>,
) -> Result<Json<CheckStateResponse>, Response> {
    let state = state
        .mint
        .check_state(&payload)
        .await
        .map_err(into_response)?;

    Ok(Json(state))
}

async fn get_mint_info(State(state): State<MintState>) -> Result<Json<MintInfo>, Response> {
    Ok(Json(state.mint.mint_info().clone()))
}

async fn post_swap(
    State(state): State<MintState>,
    Json(payload): Json<SwapRequest>,
) -> Result<Json<SwapResponse>, Response> {
    let swap_response = state
        .mint
        .process_swap_request(payload)
        .await
        .map_err(into_response)?;
    Ok(Json(swap_response))
}

async fn post_restore(
    State(state): State<MintState>,
    Json(payload): Json<RestoreRequest>,
) -> Result<Json<RestoreResponse>, Response> {
    let restore_response = state.mint.restore(payload).await.map_err(into_response)?;

    Ok(Json(restore_response))
}

pub fn into_response<T>(error: T) -> Response
where
    T: Into<ErrorResponse>,
{
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json::<ErrorResponse>(error.into()),
    )
        .into_response()
}
