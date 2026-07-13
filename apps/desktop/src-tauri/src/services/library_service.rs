use crate::domain::import_state::{
    LibraryAlbumPage, LibraryAlbumSummary, LibraryImagePage, LibraryImageSummary,
};
use crate::error::AppError;
use crate::repositories::import_repository::{
    ImportRepository, LibraryAlbumKeyset, LibraryImageKeyset,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio_postgres::Client;
use uuid::Uuid;

const MAX_PAGE_SIZE: u32 = 100;
const MAX_CURSOR_LENGTH: usize = 4096;
const ALBUM_CURSOR_KIND: &str = "library-albums";
const IMAGE_CURSOR_KIND: &str = "library-images";
const ALBUM_CURSOR_VERSION: u32 = 1;
const IMAGE_CURSOR_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct AlbumCursorEnvelope {
    kind: String,
    version: u32,
    committed_at: String,
    display_name: String,
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ImageCursorEnvelope {
    kind: String,
    version: u32,
    album_id: String,
    relative_path: String,
    id: String,
}

fn validate_limit(limit: u32) -> Result<i64, AppError> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(AppError::Internal(format!(
            "invalid page size {limit}; expected 1..={MAX_PAGE_SIZE}"
        )));
    }
    Ok(i64::from(limit) + 1)
}

fn encode_cursor<T: Serialize>(value: &T, label: &str) -> Result<String, AppError> {
    let payload = serde_json::to_vec(value)
        .map_err(|error| AppError::Internal(format!("failed to encode {label} cursor: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(payload))
}

fn decode_cursor<T: DeserializeOwned>(cursor: &str, label: &str) -> Result<T, AppError> {
    if cursor.is_empty() || cursor.len() > MAX_CURSOR_LENGTH {
        return Err(AppError::Internal(format!(
            "invalid {label} cursor: expected a non-empty opaque cursor no longer than {MAX_CURSOR_LENGTH} characters"
        )));
    }
    let payload = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|error| AppError::Internal(format!("invalid {label} cursor encoding: {error}")))?;
    serde_json::from_slice(&payload)
        .map_err(|error| AppError::Internal(format!("invalid {label} cursor payload: {error}")))
}

fn stable_timestamp(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn decode_album_cursor(cursor: Option<&str>) -> Result<Option<LibraryAlbumKeyset>, AppError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let envelope: AlbumCursorEnvelope = decode_cursor(cursor, "library album")?;
    if envelope.kind != ALBUM_CURSOR_KIND {
        return Err(AppError::Internal(format!(
            "invalid library album cursor kind {:?}; expected {ALBUM_CURSOR_KIND}",
            envelope.kind
        )));
    }
    if envelope.version != ALBUM_CURSOR_VERSION {
        return Err(AppError::Internal(format!(
            "unsupported library album cursor version {}; expected {}",
            envelope.version, ALBUM_CURSOR_VERSION
        )));
    }
    let committed_at = DateTime::parse_from_rfc3339(&envelope.committed_at)
        .map_err(|error| {
            AppError::Internal(format!(
                "invalid library album cursor timestamp {:?}: {error}",
                envelope.committed_at
            ))
        })?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(&envelope.id).map_err(|error| {
        AppError::Internal(format!(
            "invalid library album cursor id {:?}: {error}",
            envelope.id
        ))
    })?;
    Ok(Some(LibraryAlbumKeyset {
        committed_at,
        display_name: envelope.display_name,
        id,
    }))
}

fn encode_album_cursor(keyset: &LibraryAlbumKeyset) -> Result<String, AppError> {
    encode_cursor(
        &AlbumCursorEnvelope {
            kind: ALBUM_CURSOR_KIND.to_string(),
            version: ALBUM_CURSOR_VERSION,
            committed_at: stable_timestamp(&keyset.committed_at),
            display_name: keyset.display_name.clone(),
            id: keyset.id.to_string(),
        },
        "library album",
    )
}

fn decode_image_cursor(
    cursor: Option<&str>,
    album_id: Uuid,
) -> Result<Option<LibraryImageKeyset>, AppError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let envelope: ImageCursorEnvelope = decode_cursor(cursor, "library image")?;
    if envelope.kind != IMAGE_CURSOR_KIND {
        return Err(AppError::Internal(format!(
            "invalid library image cursor kind {:?}; expected {IMAGE_CURSOR_KIND}",
            envelope.kind
        )));
    }
    if envelope.version != IMAGE_CURSOR_VERSION {
        return Err(AppError::Internal(format!(
            "unsupported library image cursor version {}; expected {}",
            envelope.version, IMAGE_CURSOR_VERSION
        )));
    }
    let cursor_album_id = Uuid::parse_str(&envelope.album_id).map_err(|error| {
        AppError::Internal(format!(
            "invalid library image cursor album id {:?}: {error}",
            envelope.album_id
        ))
    })?;
    if cursor_album_id != album_id {
        return Err(AppError::Internal(format!(
            "library image cursor belongs to album {cursor_album_id}, not requested album {album_id}"
        )));
    }
    let id = Uuid::parse_str(&envelope.id).map_err(|error| {
        AppError::Internal(format!(
            "invalid library image cursor id {:?}: {error}",
            envelope.id
        ))
    })?;
    Ok(Some(LibraryImageKeyset {
        relative_path: envelope.relative_path,
        id,
    }))
}

fn encode_image_cursor(album_id: Uuid, keyset: &LibraryImageKeyset) -> Result<String, AppError> {
    encode_cursor(
        &ImageCursorEnvelope {
            kind: IMAGE_CURSOR_KIND.to_string(),
            version: IMAGE_CURSOR_VERSION,
            album_id: album_id.to_string(),
            relative_path: keyset.relative_path.clone(),
            id: keyset.id.to_string(),
        },
        "library image",
    )
}

pub async fn list_library_albums(
    client: &Client,
    cursor: Option<String>,
    limit: u32,
) -> Result<LibraryAlbumPage, AppError> {
    let db_limit = validate_limit(limit)?;
    let cursor = decode_album_cursor(cursor.as_deref())?;
    let (total_albums, total_images, total_size) =
        ImportRepository::get_library_catalog_totals(client).await?;
    let mut rows =
        ImportRepository::list_library_albums_page(client, cursor.as_ref(), db_limit).await?;
    let has_more = rows.len() > limit as usize;
    if has_more {
        rows.truncate(limit as usize);
    }
    let next_cursor = if has_more {
        rows.last()
            .map(|album| {
                encode_album_cursor(&LibraryAlbumKeyset {
                    committed_at: album.committed_at,
                    display_name: album.display_name.clone(),
                    id: album.id,
                })
            })
            .transpose()?
    } else {
        None
    };
    let albums = rows
        .into_iter()
        .map(|album| LibraryAlbumSummary {
            album_id: album.id.to_string(),
            library_root_id: album.library_root_id.to_string(),
            library_root_path: album.library_root_path,
            display_name: album.display_name,
            relative_path: album.relative_path,
            image_count: album.image_count.max(0) as u32,
            total_size: album.total_size,
            state: album.state,
            committed_at: stable_timestamp(&album.committed_at),
        })
        .collect();

    Ok(LibraryAlbumPage {
        albums,
        next_cursor,
        total_albums: total_albums.max(0) as u32,
        total_images: total_images.max(0) as u32,
        total_size,
    })
}

pub async fn list_library_images(
    client: &Client,
    album_id: Uuid,
    cursor: Option<String>,
    limit: u32,
) -> Result<LibraryImagePage, AppError> {
    let db_limit = validate_limit(limit)?;
    let cursor = decode_image_cursor(cursor.as_deref(), album_id)?;
    let (total_images, total_size) =
        ImportRepository::get_library_album_image_totals(client, album_id)
            .await?
            .ok_or_else(|| AppError::Internal(format!("library album {album_id} not found")))?;
    let mut rows =
        ImportRepository::list_library_images_page(client, album_id, cursor.as_ref(), db_limit)
            .await?;
    let has_more = rows.len() > limit as usize;
    if has_more {
        rows.truncate(limit as usize);
    }
    let next_cursor = if has_more {
        rows.last()
            .map(|image| {
                encode_image_cursor(
                    album_id,
                    &LibraryImageKeyset {
                        relative_path: image.relative_path.clone(),
                        id: image.id,
                    },
                )
            })
            .transpose()?
    } else {
        None
    };
    let images = rows
        .into_iter()
        .map(|image| LibraryImageSummary {
            image_id: image.id.to_string(),
            relative_path: image.relative_path,
            file_size: image.file_size,
            width: image.width,
            height: image.height,
            format: image.format,
            state: image.state,
        })
        .collect();

    Ok(LibraryImagePage {
        album_id: album_id.to_string(),
        images,
        next_cursor,
        total_images: total_images.max(0) as u32,
        total_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_page_size_is_bounded() {
        assert_eq!(validate_limit(50).unwrap(), 51);
        assert!(validate_limit(0).is_err());
        assert!(validate_limit(MAX_PAGE_SIZE + 1).is_err());
    }

    #[test]
    fn album_cursor_round_trips_and_rejects_unknown_sort_version() {
        let keyset = LibraryAlbumKeyset {
            committed_at: DateTime::parse_from_rfc3339("2026-07-14T10:11:12.123456Z")
                .unwrap()
                .with_timezone(&Utc),
            display_name: "同名图集".to_string(),
            id: Uuid::new_v4(),
        };
        let encoded = encode_album_cursor(&keyset).unwrap();
        let decoded = decode_album_cursor(Some(&encoded)).unwrap().unwrap();
        assert_eq!(decoded.committed_at, keyset.committed_at);
        assert_eq!(decoded.display_name, keyset.display_name);
        assert_eq!(decoded.id, keyset.id);

        let unknown_version = encode_cursor(
            &AlbumCursorEnvelope {
                kind: ALBUM_CURSOR_KIND.to_string(),
                version: ALBUM_CURSOR_VERSION + 1,
                committed_at: stable_timestamp(&keyset.committed_at),
                display_name: keyset.display_name,
                id: keyset.id.to_string(),
            },
            "library album",
        )
        .unwrap();
        let error = decode_album_cursor(Some(&unknown_version))
            .unwrap_err()
            .to_string();
        assert!(error.contains("unsupported library album cursor version"));
    }

    #[test]
    fn image_cursor_is_bound_to_its_album_and_invalid_payloads_are_clear() {
        let album_id = Uuid::new_v4();
        let keyset = LibraryImageKeyset {
            relative_path: "相册/001.jpg".to_string(),
            id: Uuid::new_v4(),
        };
        let encoded = encode_image_cursor(album_id, &keyset).unwrap();
        let decoded = decode_image_cursor(Some(&encoded), album_id)
            .unwrap()
            .unwrap();
        assert_eq!(decoded.relative_path, keyset.relative_path);
        assert_eq!(decoded.id, keyset.id);

        let wrong_album_error = decode_image_cursor(Some(&encoded), Uuid::new_v4())
            .unwrap_err()
            .to_string();
        assert!(wrong_album_error.contains("cursor belongs to album"));

        let unknown_version = encode_cursor(
            &ImageCursorEnvelope {
                kind: IMAGE_CURSOR_KIND.to_string(),
                version: IMAGE_CURSOR_VERSION + 1,
                album_id: album_id.to_string(),
                relative_path: keyset.relative_path,
                id: keyset.id.to_string(),
            },
            "library image",
        )
        .unwrap();
        let version_error = decode_image_cursor(Some(&unknown_version), album_id)
            .unwrap_err()
            .to_string();
        assert!(version_error.contains("unsupported library image cursor version"));

        let invalid_error = decode_album_cursor(Some("not-a-valid-cursor"))
            .unwrap_err()
            .to_string();
        assert!(invalid_error.contains("invalid library album cursor"));
    }
}
