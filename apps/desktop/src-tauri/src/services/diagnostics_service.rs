use crate::error::AppError;
use crate::infrastructure::postgres::MigrationRunner;
use crate::infrastructure::storage_capabilities::probe_storage_capabilities;
use crate::services::recovery_service;
use crate::state::AppState;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

const DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;
const MAX_LOG_LINES: usize = 200;
const MAX_LOG_FILES: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsExportResult {
    pub path: String,
    pub generated_at: String,
    pub file_count: u32,
    pub redacted: bool,
    pub byte_size: u64,
}

#[derive(Debug, Serialize)]
struct DiagnosticsBundle {
    schema_version: u32,
    generated_at: String,
    app_version: String,
    database: Value,
    storage_capabilities: Option<Value>,
    recent_tasks: Vec<Value>,
    migration_state: Value,
    recovery: Vec<Value>,
    logs: Vec<String>,
    redaction: Value,
}

pub async fn export_diagnostics(state: &AppState) -> Result<DiagnosticsExportResult, AppError> {
    let generated_at = Utc::now();
    let database_state = state.database_service.get_state().await;
    let settings_snapshot = {
        let settings = state.settings.lock().await;
        settings.get().clone()
    };

    let mut database = json!({
        "mode": database_state.mode.as_ref().map(ToString::to_string),
        "status": database_state.status.to_string(),
        "pgvector_available": database_state.pgvector_available,
        "schema_version": database_state.migration_version.clone(),
        "diagnostics": database_state.diagnostics,
        "managed": database_state.managed_config.as_ref().map(|c| json!({
            "data_dir": "<redacted-path>",
            "port": c.port,
            "username": c.username,
            "database": c.database,
        })),
        "external": database_state.external_config.as_ref().map(|c| json!({
            "host": c.host,
            "port": c.port,
            "database": c.database,
            "username": c.username,
            "password": "<redacted>",
            "tls_mode": c.tls_mode.as_str(),
            "ca_cert_path": redact_optional_path(c.ca_cert_path.as_deref()),
            "client_cert_path": redact_optional_path(c.client_cert_path.as_deref()),
            "client_key_path": redact_optional_path(c.client_key_path.as_deref()),
            "connect_timeout_secs": c.connect_timeout_secs,
            "query_timeout_secs": c.query_timeout_secs,
            "profile_name": c.profile_name,
        })),
    });

    let mut recent_tasks = Vec::new();
    let mut migration_state = json!({
        "latest_known_version": MigrationRunner::latest_version(),
        "current_version": null,
        "applied_versions": [],
        "status": "database_unavailable",
    });
    let mut recovery = Vec::new();

    if matches!(
        database_state.status,
        crate::domain::DatabaseStatus::Connected
    ) {
        let (client, handle) = {
            let mgr = state.postgres_manager.lock().await;
            mgr.connect().await?
        };

        if let Some(server_version) = query_single_string(&client, "SHOW server_version").await {
            database["postgres_version"] = json!(server_version);
        }
        if let Some(pgvector_version) = query_single_string(
            &client,
            "SELECT extversion FROM pg_extension WHERE extname = 'vector'",
        )
        .await
        {
            database["pgvector_version"] = json!(pgvector_version);
        }

        recent_tasks = collect_recent_tasks(&client).await;
        migration_state = collect_migration_state(&client).await;
        recovery = collect_recovery_diagnostics(&client).await;
        handle.abort();
    }

    let storage_capabilities = settings_snapshot.library_root.as_deref().map(|root| {
        let capabilities = probe_storage_capabilities(root);
        let mut value = serde_json::to_value(capabilities).unwrap_or_else(|_| json!({}));
        if let Value::Object(ref mut obj) = value {
            obj.insert("root".to_string(), json!("<redacted-path>"));
        }
        sanitize_value(value)
    });

    let logs = collect_redacted_logs(&state.app_data_dir);

    let mut bundle = DiagnosticsBundle {
        schema_version: DIAGNOSTICS_SCHEMA_VERSION,
        generated_at: generated_at.to_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        database,
        storage_capabilities,
        recent_tasks,
        migration_state,
        recovery,
        logs,
        redaction: json!({
            "secrets": "passwords, tokens, connection strings, and key paths are redacted",
            "paths": "absolute filesystem paths are replaced with <redacted-path>",
            "image_content": "image bytes and previews are never included",
        }),
    };

    bundle.database = sanitize_value(bundle.database);
    bundle.migration_state = sanitize_value(bundle.migration_state);
    bundle.recent_tasks = bundle
        .recent_tasks
        .into_iter()
        .map(sanitize_value)
        .collect();
    bundle.recovery = bundle.recovery.into_iter().map(sanitize_value).collect();

    let out_dir = state.app_data_dir.join("diagnostics");
    std::fs::create_dir_all(&out_dir)?;
    let path = out_dir.join(format!(
        "imagedb-diagnostics-{}.json",
        generated_at.format("%Y%m%dT%H%M%SZ")
    ));
    let payload = serde_json::to_vec_pretty(&bundle)
        .map_err(|e| AppError::Internal(format!("failed to serialize diagnostics: {e}")))?;
    std::fs::write(&path, payload)?;
    let byte_size = std::fs::metadata(&path)?.len();

    Ok(DiagnosticsExportResult {
        path: path.display().to_string(),
        generated_at: generated_at.to_rfc3339(),
        file_count: 1,
        redacted: true,
        byte_size,
    })
}

async fn query_single_string(client: &tokio_postgres::Client, sql: &str) -> Option<String> {
    client
        .query_opt(sql, &[])
        .await
        .ok()
        .flatten()
        .map(|row| row.get::<_, String>(0))
}

async fn collect_recent_tasks(client: &tokio_postgres::Client) -> Vec<Value> {
    let rows = client
        .query(
            "SELECT r.id::text AS id, r.state, r.started_at::text AS started_at,
                    r.completed_at::text AS completed_at, r.error_code, r.error_message,
                    COUNT(DISTINCT a.id)::BIGINT AS album_count,
                    COUNT(DISTINCT i.id)::BIGINT AS image_count,
                    COUNT(DISTINCT d.id)::BIGINT AS duplicate_count
             FROM import_runs r
             LEFT JOIN import_albums a ON a.import_run_id = r.id
             LEFT JOIN import_images i ON i.import_album_id = a.id
             LEFT JOIN duplicate_candidates d ON d.import_run_id = r.id
             GROUP BY r.id, r.state, r.started_at, r.completed_at, r.error_code, r.error_message
             ORDER BY r.started_at DESC
             LIMIT 10",
            &[],
        )
        .await;

    match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                json!({
                    "id": row.get::<_, String>("id"),
                    "state": row.get::<_, String>("state"),
                    "started_at": row.get::<_, String>("started_at"),
                    "completed_at": row.get::<_, Option<String>>("completed_at"),
                    "album_count": row.get::<_, i64>("album_count"),
                    "image_count": row.get::<_, i64>("image_count"),
                    "duplicate_count": row.get::<_, i64>("duplicate_count"),
                    "error_code": row.get::<_, Option<String>>("error_code"),
                    "error_message": row.get::<_, Option<String>>("error_message").map(|m| redact_text(&m)),
                })
            })
            .collect(),
        Err(e) => vec![json!({ "status": "unavailable", "error": redact_text(&e.to_string()) })],
    }
}

async fn collect_migration_state(client: &tokio_postgres::Client) -> Value {
    let current = MigrationRunner::current_version(client)
        .await
        .ok()
        .flatten();
    let applied = client
        .query(
            "SELECT version FROM schema_migrations ORDER BY version",
            &[],
        )
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| row.get::<_, String>("version"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "latest_known_version": MigrationRunner::latest_version(),
        "current_version": current,
        "applied_versions": applied,
        "status": "queried",
    })
}

async fn collect_recovery_diagnostics(client: &tokio_postgres::Client) -> Vec<Value> {
    match recovery_service::scan_recoverable_transactions(client).await {
        Ok(items) => items
            .into_iter()
            .map(|item| {
                json!({
                    "transaction_id": item.transaction_id.to_string(),
                    "import_run_id": item.import_run_id.to_string(),
                    "import_album_id": item.import_album_id.to_string(),
                    "current_state": item.current_state,
                    "staging_exists": item.staging_exists,
                    "target_exists": item.target_exists,
                    "manifest_exists": item.manifest_exists,
                    "has_staging_path": item.staging_path.is_some(),
                    "has_target_path": item.target_path.is_some(),
                    "has_manifest_path": item.manifest_path.is_some(),
                    "plan_hash": item.plan_hash,
                    "last_error": item.last_error.map(|m| redact_text(&m)),
                    "diagnostics": item.diagnostics.into_iter().map(|m| redact_text(&m)).collect::<Vec<_>>(),
                })
            })
            .collect(),
        Err(e) => vec![json!({ "status": "unavailable", "error": redact_text(&e.to_string()) })],
    }
}

fn collect_redacted_logs(app_data_dir: &Path) -> Vec<String> {
    let log_dir = app_data_dir.join("logs");
    let mut files = match std::fs::read_dir(&log_dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_string_lossy();
                if !name.starts_with("imagedb.log") {
                    return None;
                }
                let modified = entry.metadata().and_then(|m| m.modified()).ok();
                Some((path, modified))
            })
            .collect::<Vec<_>>(),
        Err(_) => return Vec::new(),
    };
    files.sort_by_key(|(_, modified)| *modified);
    files.reverse();

    let mut lines = Vec::new();
    for (path, _) in files.into_iter().take(MAX_LOG_FILES) {
        if let Ok(content) = std::fs::read_to_string(path) {
            let file_lines = content
                .lines()
                .rev()
                .take(MAX_LOG_LINES.saturating_sub(lines.len()))
                .map(redact_text)
                .collect::<Vec<_>>();
            lines.extend(file_lines.into_iter().rev());
        }
        if lines.len() >= MAX_LOG_LINES {
            break;
        }
    }
    lines
}

fn redact_optional_path(value: Option<&str>) -> Option<String> {
    value.map(|_| "<redacted-path>".to_string())
}

fn sanitize_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact_text(&s)),
        Value::Array(items) => Value::Array(items.into_iter().map(sanitize_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let key = k.to_ascii_lowercase();
                    if key.contains("password")
                        || key.contains("secret")
                        || key.contains("token")
                        || key.contains("key_path")
                    {
                        (k, Value::String("<redacted>".to_string()))
                    } else if key.ends_with("_path") || key == "path" || key == "root" {
                        (k, Value::String("<redacted-path>".to_string()))
                    } else {
                        (k, sanitize_value(v))
                    }
                })
                .collect(),
        ),
        other => other,
    }
}

fn redact_text(input: &str) -> String {
    let mut out = input.to_string();
    out = redact_postgres_uri(&out);
    for key in [
        "password",
        "pass",
        "pwd",
        "pgpassword",
        "secret",
        "token",
        "client_key_path",
    ] {
        out = redact_assignment(&out, key);
    }
    out = redact_path_tokens(&out);
    out
}

fn redact_postgres_uri(input: &str) -> String {
    let mut out = input.to_string();
    for scheme in ["postgres://", "postgresql://"] {
        let mut search_from = 0;
        while search_from < out.len() {
            let lower = out[search_from..].to_ascii_lowercase();
            let Some(start_rel) = lower.find(scheme) else {
                break;
            };
            let start = search_from + start_rel;
            let auth_start = start + scheme.len();
            let Some(at_rel) = out[auth_start..].find('@') else {
                break;
            };
            let at = auth_start + at_rel;
            if &out[auth_start..at] == "<redacted>" {
                search_from = at + 1;
                continue;
            }
            let Some(end_rel) =
                out[at..].find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            else {
                out.replace_range(auth_start..at, "<redacted>");
                break;
            };
            let end = at + end_rel;
            out.replace_range(auth_start..at, "<redacted>");
            search_from = end.min(out.len());
        }
    }
    out
}

fn redact_assignment(input: &str, key: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut remaining = input;
    let lower_key = key.to_ascii_lowercase();
    loop {
        let lower = remaining.to_ascii_lowercase();
        let Some(pos) = lower.find(&lower_key) else {
            result.push_str(remaining);
            break;
        };
        result.push_str(&remaining[..pos]);
        let after_key = pos + key.len();
        let Some(delim_rel) = remaining[after_key..].find(['=', ':']) else {
            result.push_str(&remaining[pos..]);
            break;
        };
        let delim = after_key + delim_rel;
        result.push_str(&remaining[pos..=delim]);
        result.push_str("<redacted>");
        let value_start = delim + 1;
        let value_end = remaining[value_start..]
            .find(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '"')
            .map(|rel| value_start + rel)
            .unwrap_or(remaining.len());
        remaining = &remaining[value_end..];
    }
    result
}

fn redact_path_tokens(input: &str) -> String {
    input
        .split_whitespace()
        .map(|token| {
            let trimmed =
                token.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';');
            let is_windows_path = trimmed.len() > 3
                && trimmed.as_bytes().get(1) == Some(&b':')
                && (trimmed.contains('\\') || trimmed.contains('/'));
            let is_unc_path = trimmed.starts_with("\\\\");
            let is_image_path = trimmed.to_ascii_lowercase().ends_with(".png")
                || trimmed.to_ascii_lowercase().ends_with(".jpg")
                || trimmed.to_ascii_lowercase().ends_with(".jpeg")
                || trimmed.to_ascii_lowercase().ends_with(".webp");
            if is_windows_path || is_unc_path || is_image_path {
                token.replace(trimmed, "<redacted-path>")
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_removes_secrets_and_image_paths() {
        let input = "postgres://user:topsecret@localhost/db password=hunter2 PGPASSWORD=wolf C:\\photos\\album\\a.png";
        let redacted = redact_text(input);
        assert!(!redacted.contains("topsecret"));
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("wolf"));
        assert!(!redacted.contains("a.png"));
        assert!(redacted.contains("<redacted>"));
        assert!(redacted.contains("<redacted-path>"));
    }

    #[test]
    fn sanitize_value_redacts_sensitive_keys() {
        let value = json!({
            "password": "secret",
            "source_path": "C:\\photos\\a.jpg",
            "nested": { "client_key_path": "C:\\keys\\client.key" }
        });
        let text = serde_json::to_string(&sanitize_value(value)).unwrap();
        assert!(!text.contains("secret"));
        assert!(!text.contains("a.jpg"));
        assert!(!text.contains("client.key"));
    }
}
