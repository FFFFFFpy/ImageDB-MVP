import type { ImportPlan, ImportPlanAlbum, ImportPlanImage } from '../../lib/ipc/types';

const albumDefinitions = [
  ['album-travel', '旅行风光', 120],
  ['album-city', '城市建筑', 110],
  ['album-pets', '可爱宠物', 100],
  ['album-life', '生活记录', 96],
  ['album-flowers', '花卉植物', 100],
  ['album-portraits', '人物肖像', 100],
] as const;

function albumImages(albumId: string, albumName: string, count: number): ImportPlanImage[] {
  return Array.from({ length: count }, (_, index) => {
    const filename = `IMG_${String(index + 1).padStart(4, '0')}.jpg`;
    return {
      image_id: `${albumId}-image-${index + 1}`,
      source_path: `D:/照片归档/2026 夏日旅行（待整理）/${albumName}/${filename}`,
      relative_path: `${albumName}/${filename}`,
      file_size: 180_000 + ((index * 73_121) % 2_400_000),
      album_name: albumName,
      album_id: albumId,
      source_album_id: albumId,
      included: true,
    };
  });
}

export function createLargeImportPlanFixture(albumCount = 1_000, imagesPerAlbum = 10): ImportPlan {
  const largeAlbums: ImportPlanAlbum[] = Array.from({ length: albumCount }, (_, albumIndex) => {
    const albumId = `stress-album-${String(albumIndex + 1).padStart(4, '0')}`;
    const albumName = `压力测试图集 ${String(albumIndex + 1).padStart(4, '0')}（中文 长路径）`;
    const images = albumImages(albumId, albumName, imagesPerAlbum);
    return {
      album_id: albumId,
      album_name: albumName,
      included: true,
      image_count: images.length,
      total_size: images.reduce((sum, image) => sum + image.file_size, 0),
      images,
    };
  });

  return {
    import_run_id: 'fixture-run-stress-plan',
    plan_hash: 'fixture-stress-plan-hash',
    source_file_mode: 'copy_and_archive',
    total_albums: albumCount,
    total_images: albumCount * imagesPerAlbum,
    kept_images: largeAlbums.flatMap((album) => album.images),
    excluded_count: 0,
    skipped_albums: [],
    albums: largeAlbums,
  };
}

const albums: ImportPlanAlbum[] = albumDefinitions.map(([albumId, albumName, count]) => {
  const images = albumImages(albumId, albumName, count);
  return {
    album_id: albumId,
    album_name: albumName,
    included: true,
    image_count: count,
    total_size: images.reduce((sum, image) => sum + image.file_size, 0),
    images,
  };
});

export const importPlanFixture: ImportPlan = {
  import_run_id: 'fixture-run-plan',
  plan_hash: 'fixture-plan-hash',
  source_file_mode: 'copy_and_archive',
  total_albums: 6,
  total_images: 808,
  kept_images: albums.flatMap((album) => album.images),
  excluded_count: 182,
  skipped_albums: [],
  albums,
};
