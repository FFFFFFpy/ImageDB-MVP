import { describe, expect, test } from 'vitest';
import { isTerminalScanState, nextRouteForScanState } from './ScanPage';

describe('ScanPage state routing', () => {
  test('treats committable and review scan states as terminal', () => {
    expect(isTerminalScanState('ready_to_commit')).toBe(true);
    expect(isTerminalScanState('review_required')).toBe(true);
    expect(isTerminalScanState('completed')).toBe(true);
    expect(isTerminalScanState('cancelled')).toBe(true);
    expect(isTerminalScanState('failed')).toBe(true);
    expect(isTerminalScanState('running')).toBe(false);
  });

  test('routes completed scan states to the next public workflow page', () => {
    expect(nextRouteForScanState('ready_to_commit')).toBe('commit');
    expect(nextRouteForScanState('review_required')).toBe('review');
    expect(nextRouteForScanState('failed')).toBeNull();
    expect(nextRouteForScanState(null)).toBeNull();
  });
});
