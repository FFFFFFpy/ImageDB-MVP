use crate::domain::{ConnectionConfig, TlsMode};
use crate::error::AppError;
use native_tls::{Certificate, TlsConnector};
use postgres_native_tls::MakeTlsConnector;
use std::future::Future;
use std::time::Duration;
use tokio_postgres::config::SslMode;
use tokio_postgres::{Client, NoTls};

pub async fn connect_external(
    config: &ConnectionConfig,
) -> Result<(Client, tokio::task::JoinHandle<()>), AppError> {
    let mut pg = tokio_postgres::Config::new();
    pg.host(&config.host)
        .port(config.port)
        .dbname(&config.database)
        .user(&config.username)
        .application_name("ImageDB");

    if let Some(password) = &config.password {
        pg.password(password);
    }

    if config.connect_timeout_secs > 0 {
        pg.connect_timeout(Duration::from_secs(config.connect_timeout_secs));
    }

    match config.tls_mode {
        TlsMode::Disable => {
            pg.ssl_mode(SslMode::Disable);
            let (client, conn) = pg.connect(NoTls).await.map_err(|e| {
                AppError::PostgresUnavailable(format!("external connection failed: {e}"))
            })?;
            Ok(spawn_connection(client, conn))
        }
        TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull => {
            pg.ssl_mode(SslMode::Require);

            let mut builder = TlsConnector::builder();
            if matches!(config.tls_mode, TlsMode::Require) {
                builder.danger_accept_invalid_certs(true);
                builder.danger_accept_invalid_hostnames(true);
            } else if matches!(config.tls_mode, TlsMode::VerifyCa) {
                builder.danger_accept_invalid_hostnames(true);
            }

            if let Some(path) = &config.ca_cert_path {
                let pem = std::fs::read(path).map_err(|e| {
                    AppError::Internal(format!("failed to read CA certificate {path}: {e}"))
                })?;
                let cert = Certificate::from_pem(&pem).map_err(|e| {
                    AppError::Internal(format!("failed to parse CA certificate {path}: {e}"))
                })?;
                builder.add_root_certificate(cert);
            }

            let connector =
                MakeTlsConnector::new(builder.build().map_err(|e| {
                    AppError::Internal(format!("failed to build TLS connector: {e}"))
                })?);

            let (client, conn) = pg.connect(connector).await.map_err(|e| {
                AppError::PostgresUnavailable(format!("external TLS connection failed: {e}"))
            })?;
            Ok(spawn_connection(client, conn))
        }
    }
}

fn spawn_connection<C>(client: Client, conn: C) -> (Client, tokio::task::JoinHandle<()>)
where
    C: Future<Output = Result<(), tokio_postgres::Error>> + Send + 'static,
{
    let handle = tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::warn!("External PostgreSQL connection lost: {e}");
        }
    });
    (client, handle)
}
