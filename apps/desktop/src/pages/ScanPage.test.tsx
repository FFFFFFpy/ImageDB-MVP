import { describe, expect, test } from 'vitest';
import {
  isTerminalScanState,
  nextActionLabelForScanState,
  nextRouteForScanState,
} from './ScanPage';

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
    expect(nextRouteForScanState('review_required')).toBe('review');
    expect(nextRouteForScanState('ready_to_commit')).toBe('review');
    expect(nextRouteForScanState('failed')).toBeNull();
    expect(nextRouteForScanState('cancelled')).toBeNull();
    expect(nextRouteForScanState('completed')).toBeNull();
    expect(nextRouteForScanState(null)).toBeNull();
  });

  test('uses review wording for ready-to-commit scan results', () => {
    expect(nextActionLabelForScanState('ready_to_commit')).toBe('前往入库审核');
    expect(nextActionLabelForScanState('review_required')).toBe('前往入库审核');
    expect(nextActionLabelForScanState('ready_to_commit')).not.toBe('前往提交');
    expect(nextActionLabelForScanState('failed')).toBeNull();
  });
});
