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
                AppError::PostgresUnavailable(format_external_connect_error(
                    "external connection failed",
                    &e,
                ))
            })?;
            configure_external_session(spawn_connection(client, conn), config).await
        }
        TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull => {
            pg.ssl_mode(SslMode::Require);

            let connector = MakeTlsConnector::new(build_tls_connector(config)?);

            let (client, conn) = pg.connect(connector).await.map_err(|e| {
                AppError::PostgresUnavailable(format_external_connect_error(
                    "external TLS connection failed",
                    &e,
                ))
            })?;
            configure_external_session(spawn_connection(client, conn), config).await
        }
    }
}

async fn configure_external_session(
    connection: (Client, tokio::task::JoinHandle<()>),
    config: &ConnectionConfig,
) -> Result<(Client, tokio::task::JoinHandle<()>), AppError> {
    let (client, handle) = connection;
    let timeout_ms = config.query_timeout_secs.saturating_mul(1_000);
    let timeout_value = format!("{timeout_ms}ms");
    let configured = client
        .query_one(
            "SELECT set_config('statement_timeout', $1, false),
                    set_config('lock_timeout', $1, false)",
            &[&timeout_value],
        )
        .await;
    if let Err(error) = configured {
        handle.abort();
        return Err(AppError::PostgresUnavailable(
            format_external_connect_error("failed to configure external query timeout", &error),
        ));
    }
    Ok((client, handle))
}

fn format_external_connect_error(context: &str, error: &tokio_postgres::Error) -> String {
    if let Some(db_error) = error.as_db_error() {
        return format!(
            "{context}: {}: {}",
            db_error.code().code(),
            db_error.message()
        );
    }
    format!("{context}: {error}")
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
    use native_tls::TlsAcceptor;
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    const TEST_LOCALHOST_CERT: &str = r#"-----BEGIN CERTIFICATE-----
MIIDBDCCAeygAwIBAgIUfUUEpAN1YcxG6Y7pp0oVNhTG+rkwDQYJKoZIhvcNAQEL
BQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDcwMzA4MDY1OVoXDTI3MDcw
NDA4MDY1OVowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF
AAOCAQ8AMIIBCgKCAQEAz9RjgOI7K0YMC5qk+XHxmE74khtGIUnUt8IWKsnfKepW
uaoOTLePdr4wQ+NjFoBST/2Y+EJqCxvm4BiJXYk8QZpKX42jEZ2vzkBNsQOuBE0v
s0Y0UU5+ldvzdCJQnQjENL8A6WhJ59WdMXkvyp6kKzhYLKQT7XiQUGfb3DaAhBra
zw5Gt+U1apZvEBeBobKMjLrTIRQN8mTF0NtsIwpzg+NSH/lX20zKrOfSeJ5JiKqP
ThKkdptq5gmCTTgl785ml3kGgbyBGhYrunuFLWV9Sa7+j2vf0NnNMeziGp5jzY4E
U94e5XdK/4YENCc2hQdvQ7+Oi66+jzSM73ARYLK6vQIDAQABo04wTDAUBgNVHREE
DTALgglsb2NhbGhvc3QwDwYDVR0TAQH/BAUwAwEB/zATBgNVHSUEDDAKBggrBgEF
BQcDATAOBgNVHQ8BAf8EBAMCAaYwDQYJKoZIhvcNAQELBQADggEBAC2ADKiGpQTv
NGdvQCCafg13FgIBoa4sUtVm5e9p2j/6F1jwv+4PihcnatmCgkLK4qdwd6o1jXPD
RArc85wg/fitETFZq1cTEaBXpaincHac1tNMtb+ZrVZRUYZcR1+eeiUIYY7k39qL
9nfVm+1yAHk4rkEfb03z2QKFPo9N1lU/yUr/LEtQAOBsCZkrjp9wdnUJJpuqwJwu
m6Ms6ZRAUMtKW3q9o91zOq6ZMb8eN81Z/OL6R/mFy40oJRTWmCyuC1vWmbi2Yifi
B4UMnDAM24HrGpft91ZgBkq6ibE0zWHOxaLl1zdEiy1zAtFpUANjr2hue1lhqlkT
TwsTf8hSNTc=
-----END CERTIFICATE-----"#;

    const TEST_LOCALHOST_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQDP1GOA4jsrRgwL
mqT5cfGYTviSG0YhSdS3whYqyd8p6la5qg5Mt492vjBD42MWgFJP/Zj4QmoLG+bg
GIldiTxBmkpfjaMRna/OQE2xA64ETS+zRjRRTn6V2/N0IlCdCMQ0vwDpaEnn1Z0x
eS/KnqQrOFgspBPteJBQZ9vcNoCEGtrPDka35TVqlm8QF4GhsoyMutMhFA3yZMXQ
22wjCnOD41If+VfbTMqs59J4nkmIqo9OEqR2m2rmCYJNOCXvzmaXeQaBvIEaFiu6
e4UtZX1Jrv6Pa9/Q2c0x7OIanmPNjgRT3h7ld0r/hgQ0JzaFB29Dv46Lrr6PNIzv
cBFgsrq9AgMBAAECgf9drP/9B3iy5mRDDE8Rf4pqdDPslWGFZw3SKyfniaJBN+xr
K3t/G9KtqJgD3dr3A323P/3zHiwEc5yGqkEodVJS4EkB46FxpBdXEQEpW8snRfCd
5TId2QaR4eQFRn42Ji0IdZhIcs+zcor1TyXmptlnZjU6bNg1SsHBlAiwsiHBOVP1
3Yam0Td0Y4VzWA84wDfyTKFSJuqKQ3K72Ccu/fsx6etLgz7UmCQCfTzwzEHZ+tim
vdvA8hariQmAbjQKNbSmwrNB0wLzkp0Wgy4JYoPATQR4QbqMETgezLX0MxjUA6x0
n28NdSEQeqsOeWgwEg06FIpQ5/jK6OVATEp5/mkCgYEA698n9FEaHVctt82OD6uX
k1J/Wi5L4SeLWESJPVWNJYzAq89kqD3fyRlVEN3GLIj/pjxtJt+ZkpCOOGyLTtm8
YaqJpofChbaHStS2XTShILBS/fFi7jv1do2aTEU3lK+FLRPgFGZ77JIjn5W81Ut5
pDIiIajhJ7hHfVQsEkhBJukCgYEA4ZCget2g3AJ8AEzo34TWV+Q/sB6/IshPQe5S
M6RrpE6Jn+fJ2uB6/OiVhYLF38kb6l1IXcd5/3hApJ16/xYVKdpR4Do8JkWCw59P
Dh36aM8eHKbrhSOUC+3rVQXUIOXVRJGJtU+jyJYcNhq2sR70hBKSz3FSwul0BzAW
OpgdeLUCgYAK3oGs1H/rkjTdH2/IcRPPCiIsOa3tdjEJpD7ewK58aHwIbsooppFF
ZxFwcYfMTZPaSTaOcAdXpamoF/hjbc0sgvtM3TythLe/TwYITYCPTRDF+vWgHMs2
51eQ5C+nfl8YsK3GwuI7CJDzrabB/XRhiJ3iBzI47lj9AX/2Z7X44QKBgQCVcjox
TXfPbMH1fP9pYFyXHP3pVWWzyN1iRGEoIA7FbNeYH31IzCQQPpUaQRuS+m7JZ4aT
w58b2POTXVdpfJsHAMPweQTzImjR7VH2e3w2RsufliRDMOBcywR5b4QtS7lyVa7U
dvB/7JzCaA6U6Xp9qsSkNmPsCbq7LGv95FzaZQKBgQCbxiYVNLYAhvWrNj2+5dOa
0nBrrtJd/eRSO0gQkjrzdSvfi1x4z228q3XX+hjich3d4OQNWIsv1B5l6x4XX+NK
e1bDxkgd3VEOsTuTtn7z/ajJmrByDAOJXHf7Owqmu4VRV6/uDdfNgLhVKRXkfE7g
svMuf5iOfkujHO2necKFjw==
-----END PRIVATE KEY-----"#;

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

    fn spawn_localhost_tls_server() -> (u16, thread::JoinHandle<()>) {
        let identity = Identity::from_pkcs8(
            TEST_LOCALHOST_CERT.as_bytes(),
            TEST_LOCALHOST_KEY.as_bytes(),
        )
        .expect("test TLS identity should parse");
        let acceptor = TlsAcceptor::new(identity).expect("test TLS acceptor should build");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test TLS server");
        let port = listener.local_addr().expect("local address").port();
        let handle = thread::spawn(move || {
            if let Ok((socket, _)) = listener.accept() {
                let _ = acceptor.accept(socket);
            }
        });
        (port, handle)
    }

    fn trusted_localhost_connector(temp: &tempfile::TempDir, tls_mode: TlsMode) -> TlsConnector {
        let ca_path = temp.path().join("localhost-ca.pem");
        std::fs::write(&ca_path, TEST_LOCALHOST_CERT).unwrap();
        let mut cfg = config();
        cfg.tls_mode = tls_mode;
        cfg.ca_cert_path = Some(ca_path.display().to_string());
        build_tls_connector(&cfg).expect("trusted test connector should build")
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

    #[test]
    fn verify_full_rejects_trusted_certificate_hostname_mismatch() {
        let temp = tempfile::TempDir::new().unwrap();
        let connector = trusted_localhost_connector(&temp, TlsMode::VerifyFull);

        let (port, server) = spawn_localhost_tls_server();
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect test TLS server");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        connector
            .connect("localhost", stream)
            .expect("trusted localhost certificate should be accepted for localhost");
        server.join().expect("localhost TLS server thread");

        let connector = trusted_localhost_connector(&temp, TlsMode::VerifyFull);
        let (port, server) = spawn_localhost_tls_server();
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect test TLS server");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        connector
            .connect("127.0.0.1", stream)
            .expect_err("verify_full must reject a trusted certificate for the wrong hostname");
        server.join().expect("mismatch TLS server thread");
    }

    #[test]
    fn verify_ca_accepts_trusted_certificate_hostname_mismatch() {
        let temp = tempfile::TempDir::new().unwrap();
        let connector = trusted_localhost_connector(&temp, TlsMode::VerifyCa);
        let (port, server) = spawn_localhost_tls_server();
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect test TLS server");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        connector
            .connect("127.0.0.1", stream)
            .expect("verify_ca should validate CA while allowing hostname mismatch");
        server.join().expect("verify_ca TLS server thread");
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_external_query_timeout_is_applied() {
        use crate::infrastructure::postgres::PostgresManager;

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            panic!("IMAGEDB_POSTGRES_BIN is required for external timeout test");
        }

        let temp = tempfile::TempDir::new().unwrap();
        let mut manager = PostgresManager::new(temp.path());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);

        let config = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: manager.port(),
            database: manager.database().to_string(),
            username: manager.username().to_string(),
            password: manager.password().map(ToString::to_string),
            tls_mode: TlsMode::Disable,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: 5,
            query_timeout_secs: 1,
            profile_name: None,
        };
        let (client, handle) = connect_external(&config).await.unwrap();
        let started = std::time::Instant::now();
        let error = client
            .query_one("SELECT pg_sleep(3)", &[])
            .await
            .expect_err("statement_timeout must cancel a slow external query");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "external query timeout was not enforced promptly"
        );
        assert_eq!(
            error.as_db_error().map(|db_error| db_error.code().code()),
            Some("57014")
        );

        drop(client);
        handle.abort();
        manager.shutdown().await.unwrap();
    }
}
