import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import { formatDiagnostic, formatTaggedStatus, taggedStatusCode } from '../lib/format';
import { useState } from 'react';
import type { ExternalConnectionConfig } from '../lib/ipc/types';

export function SettingsPage() {
  const queryClient = useQueryClient();

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: api.getSettings,
  });

  const dbStatus = useQuery({
    queryKey: ['database-status'],
    queryFn: api.getDatabaseStatus,
    refetchInterval: 5000,
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

  const migrateExt = useMutation({
    mutationFn: () => api.migrateManagedToExternalDatabase(buildExternalConfig()),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['database-status'] }),
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

  return (
    <div className="settings-page">
      <h1>设置</h1>

      <section className="settings-section">
        <h2>数据库状态</h2>
        {dbStatus.data && (
          <div className="db-status-card">
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
                {dbStatus.data.managed_config && (
                  <>
                    <tr>
                      <td>数据目录</td>
                      <td className="mono">{dbStatus.data.managed_config.data_dir}</td>
                    </tr>
                    <tr>
                      <td>端口</td>
                      <td>{dbStatus.data.managed_config.port}</td>
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
            <button onClick={() => shutdown.mutate()} disabled={shutdown.isPending}>
              {shutdown.isPending ? '停止中…' : '停止数据库'}
            </button>
          </div>
        )}
      </section>

      <section className="settings-section">
        <h2>外部数据库连接</h2>
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
            Profile 名称
            <input value={extProfileName} onChange={(e) => setExtProfileName(e.target.value)} />
          </label>
        </div>
        <button onClick={() => testExt.mutate()} disabled={testExt.isPending}>
          {testExt.isPending ? '测试中…' : '测试连接'}
        </button>
        <button
          className="btn-primary"
          onClick={() => migrateExt.mutate()}
          disabled={migrateExt.isPending}
        >
          {migrateExt.isPending ? '迁移中…' : '从托管库迁移'}
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
                  <td>Schema 权限</td>
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
        {migrateExt.data && (
          <div className="check-result">
            <table>
              <tbody>
                <tr>
                  <td>切换结果</td>
                  <td>{migrateExt.data.switched ? '已切换到外部库' : '未切换'}</td>
                </tr>
                <tr>
                  <td>备份</td>
                  <td className="mono">{migrateExt.data.backup_path ?? '未生成'}</td>
                </tr>
                <tr>
                  <td>迁移版本</td>
                  <td className="mono">{migrateExt.data.migration_version ?? '未知'}</td>
                </tr>
              </tbody>
            </table>
            {migrateExt.data.row_counts.length > 0 && (
              <table>
                <tbody>
                  {migrateExt.data.row_counts.map((row) => (
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
            {migrateExt.data.diagnostics.length > 0 && (
              <details className="diagnostics">
                <summary>迁移诊断 ({migrateExt.data.diagnostics.length})</summary>
                <ul>
                  {migrateExt.data.diagnostics.map((d, i) => (
                    <li key={i}>{formatDiagnostic(d)}</li>
                  ))}
                </ul>
              </details>
            )}
          </div>
        )}
        {migrateExt.isError && <pre className="status-err">{String(migrateExt.error)}</pre>}
      </section>

      <section className="settings-section">
        <h2>图库目录</h2>
        <label>
          目标图库根目录
          <input
            value={libRoot}
            onChange={(e) => setLibRoot(e.target.value)}
            placeholder="/path/to/library"
          />
        </label>
        <button
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
      </section>
    </div>
  );
}
