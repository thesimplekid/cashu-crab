//! Errors

use std::fmt;
use std::string::FromUtf8Error;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;

use crate::util::hex;

/// CDK Error
#[derive(Debug, Error)]
pub enum Error {
    /// Mint does not have a key for amount
    #[error("No Key for Amount")]
    AmountKey,
    /// Amount is not what expected
    #[error("Amount miss match")]
    Amount,
    /// Token is already spent
    #[error("Token already spent")]
    TokenSpent,
    /// Token could not be validated
    #[error("Token not verified")]
    TokenNotVerified,
    /// Invalid payment request
    #[error("Invalid payment request")]
    InvalidPaymentRequest,
    /// Bolt11 invoice does not have amount
    #[error("Invoice Amount undefined")]
    InvoiceAmountUndefined,
    /// Proof is missing a required field
    #[error("Proof missing required field")]
    MissingProofField,
    /// No valid point on curve
    #[error("No valid point found")]
    NoValidPoint,
    /// Secp256k1 error
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// Secret error
    #[error(transparent)]
    Secret(#[from] super::secret::Error),
    /// Bip32 error
    #[error(transparent)]
    Bip32(#[from] bitcoin::bip32::Error),
    /// Parse int error
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    /// Parse Url Error
    #[error(transparent)]
    UrlParseError(#[from] url::ParseError),
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] FromUtf8Error),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] base64::DecodeError),
    /// From hex error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    #[cfg(feature = "wallet")]
    /// From hex error
    #[error(transparent)]
    ReqwestError(#[from] reqwest::Error),
    /// Nut01 error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
    /// NUT02 error
    #[error(transparent)]
    NUT02(#[from] crate::nuts::nut02::Error),
    /// NUT11 Error
    #[error(transparent)]
    NUT11(#[from] crate::nuts::nut11::Error),
    /// Custom error
    #[error("`{0}`")]
    CustomError(String),
}

/// CDK Error Response
///
/// See NUT defenation in [00](https://github.com/cashubtc/nuts/blob/main/00.md)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Error Code
    pub code: ErrorCode,
    /// Human readable Text
    pub error: Option<String>,
    /// Longer human readable description
    pub detail: Option<String>,
}

impl fmt::Display for ErrorResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "code: {}, error: {}, detail: {}",
            self.code,
            self.error.clone().unwrap_or_default(),
            self.detail.clone().unwrap_or_default()
        )
    }
}

impl ErrorResponse {
    /// Create new [`ErrorResponse`]
    pub fn new(code: ErrorCode, error: Option<String>, detail: Option<String>) -> Self {
        Self {
            code,
            error,
            detail,
        }
    }

    /// Error response from json
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_str(json)?;

        Self::from_value(value)
    }

    /// Error response from json Value
    pub fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        match serde_json::from_value::<ErrorResponse>(value.clone()) {
            Ok(res) => Ok(res),
            Err(_) => Ok(Self {
                code: ErrorCode::Unknown(999),
                error: Some(value.to_string()),
                detail: None,
            }),
        }
    }
}

impl From<Error> for ErrorResponse {
    fn from(err: Error) -> ErrorResponse {
        match err {
            Error::TokenSpent => ErrorResponse {
                code: ErrorCode::TokenAlreadySpent,
                error: Some(err.to_string()),
                detail: None,
            },
            _ => ErrorResponse {
                code: ErrorCode::Unknown(9999),
                error: Some(err.to_string()),
                detail: None,
            },
        }
    }
}

/// Possible Error Codes
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ErrorCode {
    /// Token is already spent
    TokenAlreadySpent,
    /// Quote is not paid
    QuoteNotPaid,
    /// Keyset is not found
    KeysetNotFound,
    /// Unknown error code
    Unknown(u16),
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let code = match self {
            ErrorCode::TokenAlreadySpent => 11001,
            ErrorCode::QuoteNotPaid => 20001,
            ErrorCode::KeysetNotFound => 12001,
            ErrorCode::Unknown(code) => *code,
        };

        serializer.serialize_u16(code)
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let code = u16::deserialize(deserializer)?;

        let error_code = match code {
            11001 => ErrorCode::TokenAlreadySpent,
            20001 => ErrorCode::QuoteNotPaid,
            12001 => ErrorCode::KeysetNotFound,
            c => ErrorCode::Unknown(c),
        };

        Ok(error_code)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self {
            Self::TokenAlreadySpent => 11001,
            Self::QuoteNotPaid => 20001,
            Self::KeysetNotFound => 12001,
            Self::Unknown(code) => *code,
        };
        write!(f, "{}", code)
    }
}
