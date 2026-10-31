use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub polar_organization_id: String,
    pub polar_benefit_id: String,
    pub checkout_url: String,
    pub model: Option<Model>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub files: Vec<ModelFile>,
    pub bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelFile {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Entitlement {
    pub key: String,
    pub organization_id: String,
    pub benefit_id: String,
    pub verified_at: u64,
}
pub struct ValidationError {
    pub message: String,
    pub revoked: bool,
}
pub fn release() -> Release {
    serde_json::from_str(include_str!("../release.json")).expect("invalid release config")
}
pub fn validate_identity(v: &Value, r: &Release) -> Result<(), String> {
    if v["organization_id"] != r.polar_organization_id || v["benefit_id"] != r.polar_benefit_id {
        return Err("This license belongs to a different product.".into());
    }
    if v["status"] != "granted" {
        return Err("This license is no longer active.".into());
    }
    if !v["expires_at"].is_null() {
        return Err("Claro requires a lifetime license without an expiry date.".into());
    }
    Ok(())
}
pub fn cached() -> Option<Entitlement> {
    let entry = crate::secret("license-v1").ok()?;
    let record: Entitlement = serde_json::from_str(&entry.get_password().ok()?).ok()?;
    let config = release();
    if config.polar_organization_id.is_empty()
        || record.organization_id != config.polar_organization_id
        || record.benefit_id != config.polar_benefit_id
    {
        return None;
    }
    Some(record)
}
pub fn save(key: &str) -> Result<(), String> {
    let r = release();
    let record = Entitlement {
        key: key.into(),
        organization_id: r.polar_organization_id,
        benefit_id: r.polar_benefit_id,
        verified_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    crate::secret("license-v1")?
        .set_password(&serde_json::to_string(&record).unwrap())
        .map_err(|_| "Couldn't securely save the verified license.".into())
}
pub async fn refresh(client: &reqwest::Client) {
    if let Some(record) = cached() {
        match validate(client, &record.key).await {
            Ok(()) => {
                let _ = save(&record.key);
            }
            Err(error) if error.revoked => {
                if let Ok(entry) = crate::secret("license-v1") {
                    let _ = entry.delete_credential();
                }
            }
            Err(_) => {}
        }
    }
}
pub async fn validate(client: &reqwest::Client, key: &str) -> Result<(), ValidationError> {
    let r = release();
    if r.polar_organization_id.is_empty() || r.polar_benefit_id.is_empty() {
        return Err(ValidationError{message:"Purchasing is not available in this development build. Polar product configuration is still needed.".into(),revoked:false});
    }
    validate_at(
        client,
        key,
        &r,
        "https://api.polar.sh/v1/customer-portal/license-keys/validate",
    )
    .await
}
async fn validate_at(
    client: &reqwest::Client,
    key: &str,
    r: &Release,
    endpoint: &str,
) -> Result<(), ValidationError> {
    let response=client.post(endpoint).header("Polar-Version","2026-10").json(&json!({"key":key,"organization_id":r.polar_organization_id,"benefit_id":r.polar_benefit_id})).send().await.map_err(|_|ValidationError{message:"Cannot reach Polar. A previously verified license remains available offline.".into(),revoked:false})?;
    let status = response.status();
    if !status.is_success() {
        return Err(ValidationError {
            message: format!(
                "Polar could not validate this license (HTTP {}).",
                status.as_u16()
            ),
            revoked: matches!(status.as_u16(), 400 | 403 | 404),
        });
    }
    let body: Value = response.json().await.map_err(|_| ValidationError {
        message: "Polar returned an unreadable response.".into(),
        revoked: false,
    })?;
    validate_identity(&body, r).map_err(|message| ValidationError {
        message,
        revoked: true,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verifies_identity_and_revocation() {
        let r = Release {
            polar_organization_id: "org".into(),
            polar_benefit_id: "benefit".into(),
            checkout_url: "".into(),
            model: None,
        };
        let good = json!({"organization_id":"org","benefit_id":"benefit","status":"granted","expires_at":null});
        assert!(validate_identity(&good, &r).is_ok());
        let mut bad = good.clone();
        bad["status"] = json!("revoked");
        assert!(validate_identity(&bad, &r).is_err());
        bad = good.clone();
        bad["benefit_id"] = json!("another");
        assert!(validate_identity(&bad, &r).is_err());
    }
    #[test]
    fn exact_polar_http_contract() {
        let response=json!({"organization_id":"org","benefit_id":"benefit","status":"granted","expires_at":null}).to_string();
        let (url, receiver) = crate::test_http::server(200, &response);
        let r = Release {
            polar_organization_id: "org".into(),
            polar_benefit_id: "benefit".into(),
            checkout_url: "".into(),
            model: None,
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            assert!(
                validate_at(&reqwest::Client::new(), "test-only-license", &r, &url)
                    .await
                    .is_ok()
            );
        });
        let request = receiver.recv().unwrap();
        assert!(request.to_lowercase().contains("polar-version: 2026-10"));
        assert!(request.contains("\"benefit_id\":\"benefit\""));
        assert!(!request.to_lowercase().contains("authorization:"));
    }
}
