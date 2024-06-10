//! Mint Errors

use thiserror::Error;

use crate::cdk_database;
use crate::error::{ErrorCode, ErrorResponse};

/// CDK Mint Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Keyset
    #[error("Unknown Keyset")]
    UnknownKeySet,
    /// Inactive Keyset
    #[error("Inactive Keyset")]
    InactiveKeyset,
    /// There is not key for amount given
    #[error("No key for amount")]
    AmountKey,
    /// Amount is not what is expected
    #[error("Amount")]
    Amount,
    /// Duplicate proofs provided
    #[error("Duplicate proofs")]
    DuplicateProofs,
    /// Token is already spent
    #[error("Token Already Spent")]
    TokenAlreadySpent,
    /// Token is already pending
    #[error("Token Pending")]
    TokenPending,
    /// Quote is not paiud
    #[error("Quote not paid")]
    UnpaidQuote,
    /// Quote has already been paid
    #[error("Quote is already paid")]
    PaidQuote,
    /// Quote is not known
    #[error("Unknown quote")]
    UnknownQuote,
    /// Quote is pending
    #[error("Quote pending")]
    PendingQuote,
    /// ecash already issued for quote
    #[error("Quote already issued")]
    IssuedQuote,
    /// Unknown secret kind
    #[error("Unknown secret kind")]
    UnknownSecretKind,
    /// Multiple units provided
    #[error("Cannot have multiple units")]
    MultipleUnits,
    /// Unit not supported
    #[error("Unit not supported")]
    UnsupportedUnit,
    /// BlindMessage is already signed
    #[error("Blinded Message is already signed")]
    BlindedMessageAlreadySigned,
    /// Cashu Error
    #[error(transparent)]
    Cashu(#[from] crate::error::Error),
    /// Secret Error
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    /// NUT11 Error
    #[error(transparent)]
    NUT11(#[from] crate::nuts::nut11::Error),
    /// NUT12 Error
    #[error(transparent)]
    Nut12(#[from] crate::nuts::nut12::Error),
    /// NUT14 Error
    #[error(transparent)]
    Nut14(#[from] crate::nuts::nut14::Error),
    /// Database Error
    #[error(transparent)]
    Database(#[from] cdk_database::Error),
    /// Custom Error
    #[error("`{0}`")]
    Custom(String),
}

impl From<Error> for cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}

impl From<Error> for ErrorResponse {
    fn from(err: Error) -> ErrorResponse {
        match err {
            Error::TokenAlreadySpent => ErrorResponse {
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

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_error_response_enum() {
        let error = Error::TokenAlreadySpent;

        let response: ErrorResponse = error.into();

        let json = serde_json::to_string(&response).unwrap();

        let error_response: ErrorResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(response.code, error_response.code);
    }
}
