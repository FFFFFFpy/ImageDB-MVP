import { describe, expect, test } from 'vitest';
import type { ImportPlanImage } from '../lib/ipc/types';
import { groupImportPlanImagesByAlbum } from './ReviewPage';

function image(overrides: Partial<ImportPlanImage>): ImportPlanImage {
  return {
    image_id: overrides.image_id ?? crypto.randomUUID(),
    source_path: overrides.source_path ?? 'D:/source/image.jpg',
    relative_path: overrides.relative_path ?? 'image.jpg',
    file_size: overrides.file_size ?? 1024,
    album_name: overrides.album_name ?? 'Album',
    album_id: overrides.album_id ?? overrides.album_name ?? 'Album',
    source_album_id:
      overrides.source_album_id ?? overrides.album_id ?? overrides.album_name ?? 'Album',
    included: overrides.included ?? true,
  };
}

describe('groupImportPlanImagesByAlbum', () => {
  test('groups kept import-plan images by album and summarizes count and size', () => {
    const groups = groupImportPlanImagesByAlbum([
      image({ image_id: '1', album_name: 'Album A', relative_path: 'a.jpg', file_size: 100 }),
      image({ image_id: '2', album_name: 'Album B', relative_path: 'b.jpg', file_size: 300 }),
      image({ image_id: '3', album_name: 'Album A', relative_path: 'c.jpg', file_size: 200 }),
      image({
        image_id: '4',
        album_name: 'Album A',
        relative_path: 'skipped.jpg',
        file_size: 900,
        included: false,
      }),
    ]);

    expect(groups).toHaveLength(2);
    expect(groups[0]).toMatchObject({
      albumName: 'Album A',
      imageCount: 2,
      skippedImageCount: 1,
      totalSize: 300,
    });
    expect(groups[0].images.map((img) => img.relative_path)).toEqual([
      'a.jpg',
      'c.jpg',
      'skipped.jpg',
    ]);
    expect(groups[1]).toMatchObject({
      albumName: 'Album B',
      imageCount: 1,
      skippedImageCount: 0,
      totalSize: 300,
    });
  });
});
