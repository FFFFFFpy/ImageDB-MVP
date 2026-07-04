use crate::domain::{ConnectionConfig, TlsMode};
use crate::error::AppError;
use native_tls::{Certificate, Identity, TlsConnector};
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

            let connector = MakeTlsConnector::new(build_tls_connector(config)?);

            let (client, conn) = pg.connect(connector).await.map_err(|e| {
                AppError::PostgresUnavailable(format!("external TLS connection failed: {e}"))
            })?;
            Ok(spawn_connection(client, conn))
        }
    }
}

fn build_tls_connector(config: &ConnectionConfig) -> Result<TlsConnector, AppError> {
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
        let certs = Certificate::stack_from_pem(&pem).map_err(|e| {
            AppError::Internal(format!("failed to parse CA certificate bundle {path}: {e}"))
        })?;
        if certs.is_empty() {
            return Err(AppError::Internal(format!(
                "CA certificate bundle {path} did not contain any certificates"
            )));
        }
        for cert in certs {
            builder.add_root_certificate(cert);
        }
    }

    match (&config.client_cert_path, &config.client_key_path) {
        (Some(cert_path), Some(key_path)) => {
            let cert = std::fs::read(cert_path).map_err(|e| {
                AppError::Internal(format!(
                    "failed to read client certificate {cert_path}: {e}"
                ))
            })?;
            let key = std::fs::read(key_path).map_err(|e| {
                AppError::Internal(format!("failed to read client private key {key_path}: {e}"))
            })?;
            let identity = Identity::from_pkcs8(&cert, &key).map_err(|e| {
                AppError::Internal(format!(
                    "failed to parse client certificate/key pair {cert_path} / {key_path}: {e}. \
                     The private key must be an unencrypted PKCS#8 PEM key."
                ))
            })?;
            builder.identity(identity);
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err(AppError::Internal(
                "client certificate and private key must be provided together".to_string(),
            ));
        }
        (None, None) => {}
    }

    builder
        .build()
        .map_err(|e| AppError::Internal(format!("failed to build TLS connector: {e}")))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ConnectionConfig {
        ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: 5432,
            database: "imagedb".to_string(),
            username: "imagedb".to_string(),
            password: None,
            tls_mode: TlsMode::VerifyFull,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: 1,
            query_timeout_secs: 1,
            profile_name: None,
        }
    }

    #[test]
    fn tls_connector_rejects_missing_client_key_pair_side() {
        let mut cfg = config();
        cfg.client_cert_path = Some("client.pem".to_string());
        let err = build_tls_connector(&cfg).expect_err("cert without key should fail");
        assert!(err
            .to_string()
            .contains("client certificate and private key must be provided together"));

        let mut cfg = config();
        cfg.client_key_path = Some("client.key".to_string());
        let err = build_tls_connector(&cfg).expect_err("key without cert should fail");
        assert!(err
            .to_string()
            .contains("client certificate and private key must be provided together"));
    }

    #[test]
    fn tls_connector_rejects_bad_ca_and_client_pem() {
        let temp = tempfile::TempDir::new().unwrap();
        let ca_path = temp.path().join("ca.pem");
        std::fs::write(&ca_path, "not a certificate").unwrap();

        let mut cfg = config();
        cfg.ca_cert_path = Some(ca_path.display().to_string());
        let err = build_tls_connector(&cfg).expect_err("bad CA PEM should fail");
        let err = err.to_string();
        assert!(
            err.contains("failed to parse CA certificate bundle")
                || err.contains("did not contain any certificates"),
            "unexpected CA error: {err}"
        );

        let cert_path = temp.path().join("client.pem");
        let key_path = temp.path().join("client.key");
        std::fs::write(&cert_path, "not a certificate").unwrap();
        std::fs::write(&key_path, "not a private key").unwrap();

        let mut cfg = config();
        cfg.client_cert_path = Some(cert_path.display().to_string());
        cfg.client_key_path = Some(key_path.display().to_string());
        let err = build_tls_connector(&cfg).expect_err("bad client identity should fail");
        assert!(err
            .to_string()
            .contains("failed to parse client certificate/key pair"));
    }
}
