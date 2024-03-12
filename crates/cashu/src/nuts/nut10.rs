use std::str::FromStr;

use serde::ser::SerializeTuple;
use serde::{Deserialize, Serialize, Serializer};

use crate::error::Error;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Kind {
    /// NUT-11 P2PK
    #[default]
    P2PK,
}

#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq, Serialize)]
pub struct SecretData {
    /// Unique random string
    pub nonce: String,
    /// Expresses the spending condition specific to each kind
    pub data: String,
    /// Additional data committed to and can be used for feature extensions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq)]
pub struct Secret {
    ///  Kind of the spending condition
    pub kind: Kind,
    pub secret_data: SecretData,
}

impl Secret {
    pub fn new<S>(kind: Kind, data: S, tags: Option<Vec<Vec<String>>>) -> Self
    where
        S: Into<String>,
    {
        let nonce = crate::secret::Secret::new().to_string();
        let secret_data = SecretData {
            nonce,
            data: data.into(),
            tags,
        };

        Self { kind, secret_data }
    }
}

impl Serialize for Secret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Create a tuple representing the struct fields
        let secret_tuple = (&self.kind, &self.secret_data);

        // Serialize the tuple as a JSON array
        let mut s = serializer.serialize_tuple(2)?;

        s.serialize_element(&secret_tuple.0)?;
        s.serialize_element(&secret_tuple.1)?;
        s.end()
    }
}

impl TryFrom<Secret> for crate::secret::Secret {
    type Error = Error;
    fn try_from(secret: Secret) -> Result<crate::secret::Secret, Self::Error> {
        Ok(crate::secret::Secret::from_str(&serde_json::to_string(
            &secret,
        )?)?)
    }
}

#[cfg(test)]
mod tests {
    use std::assert_eq;

    use super::*;

    #[test]
    fn test_secret_serialize() {
        let secret = Secret {
            kind: Kind::P2PK,
            secret_data: SecretData {
                nonce: "5d11913ee0f92fefdc82a6764fd2457a".to_string(),
                data: "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"
                    .to_string(),
                tags: Some(vec![vec![
                    "key".to_string(),
                    "value1".to_string(),
                    "value2".to_string(),
                ]]),
            },
        };

        let secret_str = r#"["P2PK",{"nonce":"5d11913ee0f92fefdc82a6764fd2457a","data":"026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198","tags":[["key","value1","value2"]]}]"#;

        assert_eq!(serde_json::to_string(&secret).unwrap(), secret_str);
    }
}
