use crate::error::AppError;
use tokio_postgres::{Client, GenericClient};

// Two-key advisory locks are database-scoped in PostgreSQL. Every ImageDB
// instance uses this fixed namespace so destructive schema work can exclude
// scans, commits, recovery, and schema initialization across processes.
const IMAGEDB_OPERATION_LOCK_CLASS: i32 = 0x494D_4744; // "IMGD"
const IMAGEDB_OPERATION_LOCK_OBJECT: i32 = 0x4442_3031; // "DB01"

pub struct DatabaseOperationLock;

impl DatabaseOperationLock {
    pub async fn acquire_shared(client: &Client, operation: &str) -> Result<(), AppError> {
        let acquired: bool = client
            .query_one(
                "SELECT pg_try_advisory_lock_shared($1, $2)",
                &[
                    &IMAGEDB_OPERATION_LOCK_CLASS,
                    &IMAGEDB_OPERATION_LOCK_OBJECT,
                ],
            )
            .await
            .map_err(|error| {
                AppError::Internal(format!(
                    "failed to acquire ImageDB database lock for {operation}: {error}"
                ))
            })?
            .get(0);
        if !acquired {
            return Err(AppError::Internal(format!(
                "cannot start {operation} while another ImageDB instance is resetting or initializing this database"
            )));
        }
        Ok(())
    }

    pub async fn release_shared(client: &Client) -> Result<(), AppError> {
        let released: bool = client
            .query_one(
                "SELECT pg_advisory_unlock_shared($1, $2)",
                &[
                    &IMAGEDB_OPERATION_LOCK_CLASS,
                    &IMAGEDB_OPERATION_LOCK_OBJECT,
                ],
            )
            .await
            .map_err(|error| {
                AppError::Internal(format!(
                    "failed to release ImageDB shared database lock: {error}"
                ))
            })?
            .get(0);
        if !released {
            return Err(AppError::Internal(
                "ImageDB shared database lock was not held by this connection".to_string(),
            ));
        }
        Ok(())
    }

    pub async fn acquire_exclusive_session(
        client: &Client,
        operation: &str,
    ) -> Result<(), AppError> {
        let acquired: bool = client
            .query_one(
                "SELECT pg_try_advisory_lock($1, $2)",
                &[
                    &IMAGEDB_OPERATION_LOCK_CLASS,
                    &IMAGEDB_OPERATION_LOCK_OBJECT,
                ],
            )
            .await
            .map_err(|error| {
                AppError::Internal(format!(
                    "failed to acquire exclusive ImageDB database lock for {operation}: {error}"
                ))
            })?
            .get(0);
        if !acquired {
            return Err(AppError::Internal(format!(
                "cannot start {operation} while another ImageDB instance is using this database"
            )));
        }
        Ok(())
    }

    pub async fn release_exclusive_session(client: &Client) -> Result<(), AppError> {
        let released: bool = client
            .query_one(
                "SELECT pg_advisory_unlock($1, $2)",
                &[
                    &IMAGEDB_OPERATION_LOCK_CLASS,
                    &IMAGEDB_OPERATION_LOCK_OBJECT,
                ],
            )
            .await
            .map_err(|error| {
                AppError::Internal(format!(
                    "failed to release exclusive ImageDB database lock: {error}"
                ))
            })?
            .get(0);
        if !released {
            return Err(AppError::Internal(
                "exclusive ImageDB database lock was not held by this connection".to_string(),
            ));
        }
        Ok(())
    }

    pub async fn acquire_exclusive_transaction<C>(
        client: &C,
        operation: &str,
    ) -> Result<(), AppError>
    where
        C: GenericClient + Sync,
    {
        let acquired: bool = client
            .query_one(
                "SELECT pg_try_advisory_xact_lock($1, $2)",
                &[
                    &IMAGEDB_OPERATION_LOCK_CLASS,
                    &IMAGEDB_OPERATION_LOCK_OBJECT,
                ],
            )
            .await
            .map_err(|error| {
                AppError::Internal(format!(
                    "failed to acquire exclusive ImageDB transaction lock for {operation}: {error}"
                ))
            })?
            .get(0);
        if !acquired {
            return Err(AppError::Internal(format!(
                "cannot start {operation} while another ImageDB instance is scanning, committing, recovering, or initializing this database"
            )));
        }
        Ok(())
    }
}
