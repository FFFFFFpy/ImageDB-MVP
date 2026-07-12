import { describe, expect, test } from 'vitest';
import { createLargeImportPlanFixture } from './importPlanFixture';

describe('large import plan fixture', () => {
  test('contains 1,000 albums and 10,000 candidate images', () => {
    const plan = createLargeImportPlanFixture();

    expect(plan.total_albums).toBe(1_000);
    expect(plan.albums).toHaveLength(1_000);
    expect(plan.total_images).toBe(10_000);
    expect(plan.kept_images).toHaveLength(10_000);
    expect(plan.albums?.every((album) => album.images.length === 10)).toBe(true);
  });
});
