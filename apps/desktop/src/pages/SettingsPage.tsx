import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import { formatDiagnostic, formatTaggedStatus, taggedStatusCode } from '../lib/format';
import { useState } from 'react';
import { Button, PageHeader, StatusBadge } from '../components/ui';
import type {
  CapabilityProbe,
  DatabaseState,
  ExternalConnectionConfig,
  StorageCapabilities,
} from '../lib/ipc/types';

const capabilityLabels = {
  supported: '支持',
  unsupported: '不支持',
  unknown: '未知',
};

const strategyLabels = {
  strong_local: '强一致',
  conservative_mounted: '保守可恢复',
  unsupported: '不支持',
};

const storageTypeLabels = {
  mounted_shared: '挂载共享',
  unknown: '未知',
};

function formatMigrationState(state: string): string {
  const map: Record<string, string> = {
    idle: '空闲',
    running: '迁移中',
    completed: '已完成',
    failed: '失败',
    cancelled: '已取消',
  };
  return map[state] ?? state;
}

function formatMigrationStage(stage: string): string {
  const map: Record<string, string> = {
    idle: '空闲',
    preflight: '预检查',
    backup: '备份托管库',
    schema: '准备结构',
    copy: '复制数据',
    verify: '校验数据',
    switch_profile: '切换配置',
    completed: '已完成',
    failed: '失败',
    cancelled: '已取消',
  };
  return map[stage] ?? stage;
}

function DbActivationResult({
  state,
  successText,
}: {
  state: DatabaseState;
  successText: string | null;
}) {
  return (
    <div className="check-result">
      {successText && <p className="status-ok">{successText}</p>}
      <table>
        <tbody>
          <tr>
            <td>状态</td>
            <td>{formatTaggedStatus(state.status)}</td>
          </tr>
          <tr>
            <td>模式</td>
            <td>{state.mode ? (state.mode === 'managed_local' ? '托管' : '外部') : '未设置'}</td>
          </tr>
          <tr>
            <td>pgvector</td>
            <td>{state.pgvector_available ? '可用' : '不可用'}</td>
          </tr>
          <tr>
            <td>迁移版本</td>
            <td className="mono">{state.migration_version ?? '未执行'}</td>
          </tr>
        </tbody>
      </table>
      {state.diagnostics.length > 0 && (
        <details className="diagnostics">
          <summary>诊断信息 ({state.diagnostics.length})</summary>
          <ul>
            {state.diagnostics.map((d, i) => (
              <li key={i}>{formatDiagnostic(d)}</li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

export function SettingsPage({
  onOpenProbes,
  enablePolling = true,
}: {
  onOpenProbes?: () => void;
  enablePolling?: boolean;
}) {
  const queryClient = useQueryClient();

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: api.getSettings,
  });

  const dbStatus = useQuery({
    queryKey: ['database-status'],
    queryFn: api.getDatabaseStatus,
    refetchInterval: enablePolling ? 5000 : false,
  });

  const [extHost, setExtHost] = useState('');
  const [extPort, setExtPort] = useState('5432');
  const [extDb, setExtDb] = useState('imagedb');
  const [extUser, setExtUser] = useState('');
  const [extPass, setExtPass] = useState('');
  const [extTlsMode, setExtTlsMode] =
    useState<NonNullable<ExternalConnectionConfig['tls_mode']>>('verify_full');
  const [extCaCert, setExtCaCert] = useState('');
  const [extClientCert, setExtClientCert] = useState('');
  const [extClientKey, setExtClientKey] = useState('');
  const [extConnectTimeout, setExtConnectTimeout] = useState('10');
  const [extQueryTimeout, setExtQueryTimeout] = useState('15');
  const [extProfileName, setExtProfileName] = useState('default');
  const [libRoot, setLibRoot] = useState('');

  const saveSettings = useMutation({
    mutationFn: api.updateSettings,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['settings'] }),
  });

  const testExt = useMutation({
    mutationFn: () => api.testExternalConnection(buildExternalConfig()),
  });

  const initExternal = useMutation({
    mutationFn: () => api.initializeExternalDatabase(buildExternalConfig()),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['settings'] }),
        queryClient.invalidateQueries({ queryKey: ['database-status'] }),
      ]);
      await Promise.all([
        queryClient.refetchQueries({ queryKey: ['settings'], type: 'active' }),
        queryClient.refetchQueries({ queryKey: ['database-status'], type: 'active' }),
      ]);
    },
  });

  const migrationProgress = useQuery({
    queryKey: ['external-migration-progress'],
    queryFn: api.getExternalMigrationProgress,
    refetchInterval: enablePolling ? 1000 : false,
  });

  const startMigration = useMutation({
    mutationFn: () => api.startManagedToExternalMigration(buildExternalConfig()),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['external-migration-progress'] }),
        queryClient.invalidateQueries({ queryKey: ['database-status'] }),
        queryClient.invalidateQueries({ queryKey: ['settings'] }),
      ]);
    },
  });

  const cancelMigration = useMutation({
    mutationFn: api.cancelExternalMigration,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['external-migration-progress'] }),
  });

  const probeStorage = useMutation({
    mutationFn: () => api.probeStorageCapabilities(libRoot || settings.data?.library_root || ''),
  });

  function buildExternalConfig(): ExternalConnectionConfig {
    return {
      host: extHost,
      port: parseInt(extPort, 10),
      database: extDb,
      username: extUser,
      password: extPass || undefined,
      tls_mode: extTlsMode,
      ca_cert_path: extCaCert || null,
      client_cert_path: extClientCert || null,
      client_key_path: extClientKey || null,
      connect_timeout_secs: parseInt(extConnectTimeout, 10),
      query_timeout_secs: parseInt(extQueryTimeout, 10),
      profile_name: extProfileName || null,
    };
  }

  const shutdown = useMutation({
    mutationFn: api.shutdownDatabase,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['database-status'] }),
  });

  const switchManaged = useMutation({
    mutationFn: api.switchToManagedDatabase,
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['settings'] }),
        queryClient.invalidateQueries({ queryKey: ['database-status'] }),
      ]);
      await Promise.all([
        queryClient.refetchQueries({ queryKey: ['settings'], type: 'active' }),
        queryClient.refetchQueries({ queryKey: ['database-status'], type: 'active' }),
      ]);
    },
  });

  const exportDiagnostics = useMutation({
    mutationFn: api.exportDiagnostics,
  });

  const migration = migrationProgress.data;
  const migrationRunning = migration?.state === 'running' || startMigration.isPending;
  const effectiveLibraryRoot = libRoot || settings.data?.library_root || '';
  const currentDbMode = dbStatus.data?.mode;
  const externalConfig = dbStatus.data?.external_config;
  const managedConfig = dbStatus.data?.managed_config;

  return (
    <div className="settings-page settings-page--m3">
      <PageHeader
        title="设置"
        description="管理数据库、图库目录与诊断工具。日常导入不需要修改高级选项。"
        meta={
          dbStatus.data ? (
            <StatusBadge
              tone={taggedStatusCode(dbStatus.data.status) === 'connected' ? 'success' : 'warning'}
            >
              {formatTaggedStatus(dbStatus.data.status)}
            </StatusBadge>
          ) : undefined
        }
        actions={
          onOpenProbes ? (
            <Button variant="quiet" onClick={onOpenProbes}>
              打开技术探针
            </Button>
          ) : undefined
        }
      />

      <section className="settings-section">
        <div className="settings-section-heading">
          <div>
            <span>数据库</span>
            <h2>连接与运行状态</h2>
          </div>
          <p>托管本地 PostgreSQL 是推荐模式；切换或停止数据库会影响当前任务。</p>
        </div>
        {dbStatus.data && (
          <div className="db-status-card">
            {!dbStatus.data.mode && (
              <p className="settings-help">
                数据库模式尚未选择。可以初始化托管库，或在下方填写外部 PostgreSQL 后连接并初始化。
              </p>
            )}
            <table>
              <tbody>
                <tr>
                  <td>状态</td>
                  <td
                    className={
                      taggedStatusCode(dbStatus.data.status) === 'connected' ? 'status-ok' : ''
                    }
                  >
                    {formatTaggedStatus(dbStatus.data.status)}
                  </td>
                </tr>
                <tr>
                  <td>模式</td>
                  <td>
                    {dbStatus.data.mode
                      ? dbStatus.data.mode === 'managed_local'
                        ? '托管'
                        : '外部'
                      : '未设置'}
                  </td>
                </tr>
                <tr>
                  <td>pgvector</td>
                  <td>{dbStatus.data.pgvector_available ? '可用' : '不可用'}</td>
                </tr>
                <tr>
                  <td>迁移版本</td>
                  <td className="mono">{dbStatus.data.migration_version ?? '未执行'}</td>
                </tr>
                {currentDbMode === 'external' && externalConfig && (
                  <>
                    <tr>
                      <td>外部数据库</td>
                      <td className="mono">
                        {externalConfig.host}:{externalConfig.port}/{externalConfig.database}
                      </td>
                    </tr>
                    <tr>
                      <td>连接用户</td>
                      <td>{externalConfig.username}</td>
                    </tr>
                  </>
                )}
                {currentDbMode === 'managed_local' && managedConfig && (
                  <>
                    <tr>
                      <td>托管库数据目录</td>
                      <td className="mono">{managedConfig.data_dir}</td>
                    </tr>
                    <tr>
                      <td>端口</td>
                      <td>{managedConfig.port}</td>
                    </tr>
                  </>
                )}
                {!currentDbMode && managedConfig && (
                  <>
                    <tr>
                      <td>托管库预留目录</td>
                      <td className="mono">{managedConfig.data_dir}</td>
                    </tr>
                    <tr>
                      <td>说明</td>
                      <td>尚未使用本地托管库；连接外部库成功后会显示外部数据库。</td>
                    </tr>
                  </>
                )}
              </tbody>
            </table>
            {dbStatus.data.diagnostics.length > 0 && (
              <details className="diagnostics">
                <summary>诊断信息 ({dbStatus.data.diagnostics.length})</summary>
                <ul>
                  {dbStatus.data.diagnostics.map((d, i) => (
                    <li key={i}>{formatDiagnostic(d)}</li>
                  ))}
                </ul>
              </details>
            )}
            <button
              className="settings-danger-action"
              onClick={() => shutdown.mutate()}
              disabled={shutdown.isPending}
            >
              {shutdown.isPending ? '停止中…' : '停止数据库'}
            </button>
            <button onClick={() => switchManaged.mutate()} disabled={switchManaged.isPending}>
              {switchManaged.isPending
                ? '初始化中…'
                : currentDbMode
                  ? '切回托管库'
                  : '初始化托管库'}
            </button>
            <button
              onClick={() => exportDiagnostics.mutate()}
              disabled={exportDiagnostics.isPending}
            >
              {exportDiagnostics.isPending ? '导出中...' : '导出诊断'}
            </button>
            {switchManaged.isError && (
              <pre className="status-err">{String(switchManaged.error)}</pre>
            )}
            {switchManaged.data && (
              <DbActivationResult
                state={switchManaged.data}
                successText={
                  taggedStatusCode(switchManaged.data.status) === 'connected'
                    ? '托管库已初始化并激活。'
                    : null
                }
              />
            )}
            {exportDiagnostics.data && (
              <div className="check-result">
                <table>
                  <tbody>
                    <tr>
                      <td>诊断包</td>
                      <td className="mono">{exportDiagnostics.data.path}</td>
                    </tr>
                    <tr>
                      <td>敏感信息已隐藏</td>
                      <td>{exportDiagnostics.data.redacted ? '是' : '否'}</td>
                    </tr>
                  </tbody>
                </table>
              </div>
            )}
            {exportDiagnostics.isError && (
              <pre className="status-err">{String(exportDiagnostics.error)}</pre>
            )}
          </div>
        )}
      </section>

      <section className="settings-section">
        <div className="settings-section-heading">
          <div>
            <span>高级</span>
            <h2>外部数据库连接</h2>
          </div>
          <p>仅在已有数据库环境或需要迁移托管库时配置。</p>
        </div>
        <div className="form-grid">
          <label>
            主机
            <input value={extHost} onChange={(e) => setExtHost(e.target.value)} />
          </label>
          <label>
            端口
            <input type="number" value={extPort} onChange={(e) => setExtPort(e.target.value)} />
          </label>
          <label>
            数据库名
            <input value={extDb} onChange={(e) => setExtDb(e.target.value)} />
          </label>
          <label>
            用户名
            <input value={extUser} onChange={(e) => setExtUser(e.target.value)} />
          </label>
          <label>
            密码
            <input type="password" value={extPass} onChange={(e) => setExtPass(e.target.value)} />
          </label>
          <label>
            TLS 模式
            <select
              value={extTlsMode}
              onChange={(e) =>
                setExtTlsMode(e.target.value as NonNullable<ExternalConnectionConfig['tls_mode']>)
              }
            >
              <option value="verify_full">验证 CA 和主机名</option>
              <option value="verify_ca">验证 CA</option>
              <option value="require">仅要求加密</option>
              <option value="disable">禁用</option>
            </select>
          </label>
          <label>
            CA 证书路径
            <input value={extCaCert} onChange={(e) => setExtCaCert(e.target.value)} />
          </label>
          <label>
            客户端证书路径
            <input value={extClientCert} onChange={(e) => setExtClientCert(e.target.value)} />
          </label>
          <label>
            客户端私钥路径
            <input value={extClientKey} onChange={(e) => setExtClientKey(e.target.value)} />
          </label>
          <label>
            连接超时（秒）
            <input
              type="number"
              value={extConnectTimeout}
              onChange={(e) => setExtConnectTimeout(e.target.value)}
            />
          </label>
          <label>
            查询超时（秒）
            <input
              type="number"
              value={extQueryTimeout}
              onChange={(e) => setExtQueryTimeout(e.target.value)}
            />
          </label>
          <label>
            配置名称
            <input value={extProfileName} onChange={(e) => setExtProfileName(e.target.value)} />
          </label>
        </div>
        <button onClick={() => testExt.mutate()} disabled={testExt.isPending}>
          {testExt.isPending ? '测试中…' : '测试连接'}
        </button>
        <button
          className="btn-primary"
          onClick={() => initExternal.mutate()}
          disabled={initExternal.isPending}
        >
          {initExternal.isPending ? '连接中…' : '连接并初始化外部库'}
        </button>
        <button
          className="settings-secondary-action"
          onClick={() => startMigration.mutate()}
          disabled={migrationRunning}
        >
          {migrationRunning ? '迁移中…' : '从托管库迁移'}
        </button>
        <button
          onClick={() => cancelMigration.mutate()}
          disabled={!migrationRunning || cancelMigration.isPending}
        >
          {cancelMigration.isPending ? '取消中…' : '取消迁移'}
        </button>
        {testExt.data && (
          <div className="check-result">
            <table>
              <tbody>
                <tr>
                  <td>连接</td>
                  <td>{testExt.data.connection_ok ? '成功' : '失败'}</td>
                </tr>
                <tr>
                  <td>版本</td>
                  <td>{testExt.data.version ?? '未知'}</td>
                </tr>
                <tr>
                  <td>pgvector</td>
                  <td>{testExt.data.pgvector_available ? '可用' : '不可用'}</td>
                </tr>
                <tr>
                  <td>TLS</td>
                  <td>{testExt.data.tls_ok ? '启用' : '禁用或失败'}</td>
                </tr>
                <tr>
                  <td>建表权限</td>
                  <td>{testExt.data.can_create_tables ? '有' : '无'}</td>
                </tr>
                <tr>
                  <td>模式权限</td>
                  <td>{testExt.data.can_modify_schema ? '有' : '无'}</td>
                </tr>
                <tr>
                  <td>读写</td>
                  <td>{testExt.data.read_write_ok ? '可写' : '不可写'}</td>
                </tr>
                <tr>
                  <td>迁移状态</td>
                  <td>{testExt.data.migration_state_ok ? '兼容' : '不兼容'}</td>
                </tr>
              </tbody>
            </table>
            {testExt.data.checks.length > 0 && (
              <table>
                <tbody>
                  {testExt.data.checks.map((check) => (
                    <tr key={check.code}>
                      <td className="mono">{check.code}</td>
                      <td>
                        {check.status === 'pass'
                          ? '通过'
                          : check.status === 'warn'
                            ? '警告'
                            : '失败'}
                      </td>
                      <td>{check.message}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        )}
        {testExt.isError && <pre className="status-err">{String(testExt.error)}</pre>}
        {initExternal.data && (
          <DbActivationResult
            state={initExternal.data}
            successText={
              taggedStatusCode(initExternal.data.status) === 'connected'
                ? '外部数据库已连接并初始化。'
                : null
            }
          />
        )}
        {initExternal.isError && <pre className="status-err">{String(initExternal.error)}</pre>}
        {migration && migration.state !== 'idle' && (
          <div className="check-result">
            <table>
              <tbody>
                <tr>
                  <td>状态</td>
                  <td>{formatMigrationState(migration.state)}</td>
                </tr>
                <tr>
                  <td>阶段</td>
                  <td>{formatMigrationStage(migration.current_stage)}</td>
                </tr>
                <tr>
                  <td>切换结果</td>
                  <td>{migration.switched ? '已切换到外部库' : '未切换'}</td>
                </tr>
                <tr>
                  <td>备份</td>
                  <td className="mono">{migration.backup_path ?? '未生成'}</td>
                </tr>
                <tr>
                  <td>迁移版本</td>
                  <td className="mono">{migration.migration_version ?? '未知'}</td>
                </tr>
                <tr>
                  <td>取消请求</td>
                  <td>{migration.cancel_requested ? '已请求' : '无'}</td>
                </tr>
              </tbody>
            </table>
            {migration.row_counts.length > 0 && (
              <table>
                <tbody>
                  {migration.row_counts.map((row) => (
                    <tr key={row.table}>
                      <td className="mono">{row.table}</td>
                      <td>{row.managed_rows}</td>
                      <td>{row.external_rows}</td>
                      <td>{row.matches ? '一致' : '不一致'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {migration.diagnostics.length > 0 && (
              <details className="diagnostics">
                <summary>迁移诊断 ({migration.diagnostics.length})</summary>
                <ul>
                  {migration.diagnostics.map((d, i) => (
                    <li key={i}>{formatDiagnostic(d)}</li>
                  ))}
                </ul>
              </details>
            )}
            {migration.errors.length > 0 && (
              <pre className="status-err">{migration.errors.join('\n')}</pre>
            )}
          </div>
        )}
        {startMigration.isError && <pre className="status-err">{String(startMigration.error)}</pre>}
        {cancelMigration.isError && (
          <pre className="status-err">{String(cancelMigration.error)}</pre>
        )}
      </section>

      <section className="settings-section">
        <div className="settings-section-heading">
          <div>
            <span>存储</span>
            <h2>图库目录</h2>
          </div>
          <p>保存目标根目录，并在首次正式入库前检测文件系统能力。</p>
        </div>
        <label>
          目标图库根目录
          <input
            value={libRoot}
            onChange={(e) => setLibRoot(e.target.value)}
            placeholder="例如 D:\ImageLibrary"
          />
        </label>
        <button
          className="settings-primary-action"
          onClick={() => {
            if (settings.data) {
              saveSettings.mutate({
                ...settings.data,
                library_root: libRoot || null,
              });
            }
          }}
          disabled={saveSettings.isPending}
        >
          保存
        </button>
        <button
          onClick={() => probeStorage.mutate()}
          disabled={probeStorage.isPending || !effectiveLibraryRoot}
        >
          {probeStorage.isPending ? '检测中…' : '检测存储能力'}
        </button>
        {probeStorage.data && <StorageCapabilityReport capabilities={probeStorage.data} />}
        {probeStorage.isError && <pre className="status-err">{String(probeStorage.error)}</pre>}
      </section>

      {onOpenProbes && (
        <section className="settings-section">
          <div className="settings-section-heading">
            <div>
              <span>支持</span>
              <h2>诊断与技术探针</h2>
            </div>
            <p>导出的诊断包会隐藏敏感连接信息。</p>
          </div>
          <p className="settings-help">
            运行
            PostgreSQL、图片指纹和文件事务技术探针。这里的工具用于排查环境问题，不属于日常导入流程。
          </p>
        </section>
      )}

      <section className="settings-section settings-about">
        <div className="settings-section-heading">
          <div>
            <span>About</span>
            <h2>ImageDB M3</h2>
          </div>
          <p>个人自用、非商业项目。</p>
        </div>
        <p>
          界面组件使用{' '}
          <a href="https://github.com/guokaigdg/animal-island-ui" target="_blank" rel="noreferrer">
            animal-island-ui
          </a>
          ，作者 guokaigdg，按 CC BY-NC 4.0 许可使用。完整声明见{' '}
          <span className="mono">docs/MVP3/THIRD_PARTY_NOTICES.md</span>。
        </p>
      </section>
    </div>
  );
}

function StorageCapabilityReport({ capabilities }: { capabilities: StorageCapabilities }) {
  const rows: Array<[string, CapabilityProbe]> = [
    ['可读', capabilities.readable],
    ['可写', capabilities.writable],
    ['创建目录', capabilities.can_create_dir],
    ['文件重命名', capabilities.same_dir_file_rename],
    ['同根重命名', capabilities.same_root_rename],
    ['目录重命名', capabilities.directory_rename],
    ['覆盖重命名', capabilities.overwrite_rename],
    ['文件同步', capabilities.file_sync_all],
    ['父目录同步', capabilities.parent_dir_sync],
    ['大小写敏感', capabilities.case_sensitive],
    ['Unicode 规范化', capabilities.unicode_normalization],
    ['长路径', capabilities.max_path],
    ['长文件名', capabilities.max_component],
    ['文件锁', capabilities.file_lock],
    ['时间戳精度', capabilities.timestamp_precision],
    ['可用空间', capabilities.free_space],
    ['卷身份', capabilities.volume_identity],
  ];

  return (
    <div className="check-result">
      <table>
        <tbody>
          <tr>
            <td>路径</td>
            <td className="mono">{capabilities.root}</td>
          </tr>
          <tr>
            <td>存储类型</td>
            <td>{storageTypeLabels[capabilities.storage_type]}</td>
          </tr>
          <tr>
            <td>发布策略</td>
            <td>{strategyLabels[capabilities.publish_strategy]}</td>
          </tr>
          <tr>
            <td>临时目录清理</td>
            <td>{capabilities.probe_dir_cleaned ? '完成' : '未完成'}</td>
          </tr>
          {rows.map(([label, probe]) => (
            <tr key={label}>
              <td>{label}</td>
              <td>
                {capabilityLabels[probe.status]} · {probe.detail}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {capabilities.strategy_reasons.length > 0 && (
        <details className="diagnostics" open>
          <summary>策略依据 ({capabilities.strategy_reasons.length})</summary>
          <ul>
            {capabilities.strategy_reasons.map((reason) => (
              <li key={reason}>{reason}</li>
            ))}
          </ul>
        </details>
      )}
      {capabilities.diagnostics.length > 0 && (
        <details className="diagnostics">
          <summary>存储诊断 ({capabilities.diagnostics.length})</summary>
          <ul>
            {capabilities.diagnostics.map((d) => (
              <li key={d}>{d}</li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
