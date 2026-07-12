import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, test, vi } from 'vitest';
import {
  invalidateRecoveryWorkflowQueries,
  RecoveryPage,
  recoveryDisposition,
} from './RecoveryPage';

const mockApi = vi.hoisted(() => ({
  scanRecoverableTransactions: vi.fn(),
  recoverTransaction: vi.fn(),
  reverifyTransaction: vi.fn(),
}));

vi.mock('../lib/ipc/api', () => ({ api: mockApi }));

function renderRecoveryPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <RecoveryPage onNavigate={vi.fn()} />
    </QueryClientProvider>,
  );
}

describe('RecoveryPage transaction disposition', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockApi.scanRecoverableTransactions.mockResolvedValue([
      {
        transaction_id: '11111111-1111-1111-1111-111111111111',
        import_run_id: '22222222-2222-2222-2222-222222222222',
        import_album_id: '33333333-3333-3333-3333-333333333333',
        current_state: 'failed',
        staging_path: null,
        target_path: null,
        manifest_path: null,
        staging_exists: false,
        target_exists: false,
        manifest_exists: false,
        plan_hash: null,
        last_error: 'terminal failure',
        diagnostics: ['terminal state failed: manual resolution required'],
      },
    ]);
  });

  test('shows terminal failed transactions as manual work instead of an empty recovery state', async () => {
    renderRecoveryPage();

    expect(await screen.findByText('11111111…')).toBeInTheDocument();
    expect(screen.queryByText('没有待处理事务')).not.toBeInTheDocument();
    expect(screen.getByText('失败')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '自动恢复不可用' })).toBeDisabled();
    expect(screen.getByText('错误: terminal failure')).toBeInTheDocument();
  });

  test('keeps conflicts, terminal states, and recoverable states distinct', () => {
    expect(recoveryDisposition('conflict')).toBe('conflict');
    expect(recoveryDisposition('failed')).toBe('terminal');
    expect(recoveryDisposition('cancelled')).toBe('terminal');
    expect(recoveryDisposition('cleanup_required')).toBe('recoverable');
    expect(recoveryDisposition('staging')).toBe('recoverable');
  });

  test('refreshes recovery, navigation counts, and dashboard state together', () => {
    const queryClient = { invalidateQueries: vi.fn() };

    invalidateRecoveryWorkflowQueries(queryClient);

    expect(queryClient.invalidateQueries).toHaveBeenCalledWith({
      queryKey: ['recoverableTransactions'],
    });
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith({
      queryKey: ['database-info-dashboard'],
    });
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith({
      queryKey: ['import-runs-dashboard'],
    });
  });
});
