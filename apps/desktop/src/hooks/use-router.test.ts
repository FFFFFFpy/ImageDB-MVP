import { expect, test } from 'vitest';
import { parseRouteHash } from './use-router';

test('parses an explicit workflow runId from the hash', () => {
  expect(parseRouteHash('#/plan?runId=run-123')).toEqual({
    route: 'plan',
    runId: 'run-123',
    fresh: false,
  });
});

test('distinguishes an explicit fresh import from resolver-based scan routing', () => {
  expect(parseRouteHash('#/scan?fresh=1')).toEqual({
    route: 'scan',
    runId: null,
    fresh: true,
  });
});
