#![allow(dead_code)]

pub mod import_repository;

use crate::error::AppError;
use serde_json::Value;
use tokio_postgres::Client;

pub struct AppMetaRepository;

impl AppMetaRepository {
    pub async fn get(client: &Client, key: &str) -> Result<Option<Value>, AppError> {
        let row = client
            .query_opt("SELECT value FROM app_meta WHERE key = $1", &[&key])
            .await
            .map_err(|e| AppError::Internal(format!("failed to query app_meta: {e}")))?;

        Ok(row.map(|r| r.get::<_, Value>("value")))
    }

    pub async fn set(client: &Client, key: &str, value: &Value) -> Result<(), AppError> {
        client
            .execute(
                "INSERT INTO app_meta (key, value, updated_at)
                 VALUES ($1, $2, now())
                 ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
                &[&key, value],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to upsert app_meta: {e}")))?;
        Ok(())
    }

    pub async fn delete(client: &Client, key: &str) -> Result<bool, AppError> {
        let count = client
            .execute("DELETE FROM app_meta WHERE key = $1", &[&key])
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete app_meta: {e}")))?;
        Ok(count > 0)
    }
}
