import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { RecoveryDiagnostic } from '../../lib/ipc/types';
import { RecoveryPage } from '../../pages/RecoveryPage';
import { Layout } from '../Layout';

const transactions: RecoveryDiagnostic[] = [
  {
    transaction_id: 'a1111111-1111-1111-1111-111111111111',
    import_run_id: 'run-recovery-1',
    import_album_id: 'album-recovery-1',
    current_state: 'cleanup_required',
    source_file_mode: 'copy_and_archive',
    staging_path: 'D:/ImageDB/.staging/a1111111',
    target_path: 'D:/ImageLibrary/城市建筑',
    manifest_path: 'D:/ImageLibrary/城市建筑/.imagedb-manifest.json',
    staging_exists: true,
    target_exists: false,
    manifest_exists: true,
    plan_hash: 'a7f431bb73e84c914f28a17158fb66b2',
    last_error: '发布前进程退出，可以从已验证暂存区继续。',
    diagnostics: ['staging 与 manifest 校验一致', '目标目录尚未发布，可安全恢复'],
  },
  {
    transaction_id: 'b2222222-2222-2222-2222-222222222222',
    import_run_id: 'run-recovery-2',
    import_album_id: 'album-recovery-2',
    current_state: 'conflict',
    source_file_mode: 'move_selected_without_backup',
    staging_path: 'D:/ImageDB/.staging/b2222222',
    target_path: 'D:/ImageLibrary/花卉植物',
    manifest_path: null,
    staging_exists: true,
    target_exists: true,
    manifest_exists: false,
    plan_hash: 'f938aa2d8e6b4247b55a76150be0c672',
    last_error: '目标目录已存在，但缺少可验证清单。',
    diagnostics: ['无法确认目标目录所有权', '自动覆盖已禁用'],
  },
  {
    transaction_id: 'c3333333-3333-3333-3333-333333333333',
    import_run_id: 'run-recovery-3',
    import_album_id: 'album-recovery-3',
    current_state: 'failed',
    source_file_mode: 'copy_and_archive',
    staging_path: null,
    target_path: 'D:/ImageLibrary/人物肖像',
    manifest_path: null,
    staging_exists: false,
    target_exists: false,
    manifest_exists: false,
    plan_hash: null,
    last_error: '事务证据不完整，无法继续自动恢复。',
    diagnostics: ['terminal state failed', '需要人工保留日志并检查源图集'],
  },
];

const fixtureClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Infinity, retry: false } },
});

fixtureClient.setQueryData(['database-info-dashboard'], {
  imports: { failed_album_count: 1, pending_review_count: 0, recovery_required_run_count: 3 },
});

export function RecoveryFixture() {
  return (
    <QueryClientProvider client={fixtureClient}>
      <Layout currentRoute="recovery" onNavigate={() => undefined} enablePolling={false}>
        <RecoveryPage
          initialDiagnostics={transactions}
          enablePolling={false}
          onNavigate={() => undefined}
        />
      </Layout>
    </QueryClientProvider>
  );
}
