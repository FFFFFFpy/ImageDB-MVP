use crate::domain::import_state::{
    LibraryAlbumPage, LibraryAlbumSummary, LibraryImagePage, LibraryImageSummary,
};
use crate::error::AppError;
use crate::repositories::import_repository::ImportRepository;
use tokio_postgres::Client;
use uuid::Uuid;

const MAX_PAGE_SIZE: u32 = 100;

fn validate_page(offset: u32, limit: u32) -> Result<(i64, i64), AppError> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(AppError::Internal(format!(
            "invalid page size {limit}; expected 1..={MAX_PAGE_SIZE}"
        )));
    }
    Ok((i64::from(offset), i64::from(limit)))
}

pub async fn list_library_albums(
    client: &Client,
    offset: u32,
    limit: u32,
) -> Result<LibraryAlbumPage, AppError> {
    let (db_offset, db_limit) = validate_page(offset, limit)?;
    let (total_albums, total_images, total_size) =
        ImportRepository::get_library_catalog_totals(client).await?;
    let albums = ImportRepository::list_library_albums_page(client, db_offset, db_limit)
        .await?
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
            committed_at: album.committed_at.to_rfc3339(),
        })
        .collect();

    Ok(LibraryAlbumPage {
        albums,
        total_albums: total_albums.max(0) as u32,
        total_images: total_images.max(0) as u32,
        total_size,
        offset,
        limit,
    })
}

pub async fn list_library_images(
    client: &Client,
    album_id: Uuid,
    offset: u32,
    limit: u32,
) -> Result<LibraryImagePage, AppError> {
    let (db_offset, db_limit) = validate_page(offset, limit)?;
    let (total_images, total_size) =
        ImportRepository::get_library_album_image_totals(client, album_id)
            .await?
            .ok_or_else(|| AppError::Internal(format!("library album {album_id} not found")))?;
    let images = ImportRepository::list_library_images_page(client, album_id, db_offset, db_limit)
        .await?
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
        total_images: total_images.max(0) as u32,
        total_size,
        offset,
        limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_page_size_is_bounded() {
        assert_eq!(validate_page(0, 50).unwrap(), (0, 50));
        assert!(validate_page(0, 0).is_err());
        assert!(validate_page(0, MAX_PAGE_SIZE + 1).is_err());
    }
}
