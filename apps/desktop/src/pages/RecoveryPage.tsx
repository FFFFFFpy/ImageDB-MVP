import { useCallback, useEffect, useState } from 'react';
import type { Route } from '../hooks/use-router';

interface RecoveryPageProps {
  onNavigate: (route: Route) => void;
}

interface TransactionDiagnostic {
  transaction_id: string;
  import_run_id: string;
  import_album_id: string;
  current_state: string;
  staging_path: string | null;
  target_path: string | null;
  manifest_path: string | null;
  staging_exists: boolean;
  target_exists: boolean;
  manifest_exists: boolean;
  diagnostics: string[];
}

function formatState(state: string): string {
  const map: Record<string, string> = {
    planned: 'Planned',
    staging: 'Staging',
    verifying: 'Verifying',
    verified: 'Verified',
    publishing: 'Publishing',
    published: 'Published',
    db_committing: 'DB Committing',
    library_committed: 'Library Committed',
    source_archiving: 'Source Archiving',
    source_archived: 'Source Archived',
    cleanup_required: 'Cleanup Required',
    conflict: 'Conflict',
    failed: 'Failed',
    cancelled: 'Cancelled',
  };
  return map[state] ?? state;
}

export function RecoveryPage({ onNavigate }: RecoveryPageProps) {
  const [transactions, setTransactions] = useState<TransactionDiagnostic[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [recovering, setRecovering] = useState<string | null>(null);

  useEffect(() => {
    // In a full implementation, this would call a Tauri command
    // to scan recoverable transactions. For now, show a placeholder.
    setLoading(false);
  }, []);

  const handleRecover = useCallback(async (txId: string) => {
    setRecovering(txId);
    try {
      // await api.recoverTransaction(txId);
      setRecovering(null);
    } catch (err) {
      setError(String(err));
      setRecovering(null);
    }
  }, []);

  if (loading) {
    return (
      <div className="recovery-page">
        <h1>Recovery</h1>
        <p>Scanning for recoverable transactions...</p>
      </div>
    );
  }

  return (
    <div className="recovery-page">
      <h1>Recovery</h1>

      {error && <div className="recovery-error">{error}</div>}

      {transactions.length === 0 ? (
        <div className="empty-state">
          <h1>No Recoverable Transactions</h1>
          <p>All file transactions are in a completed or terminal state.</p>
          <button className="btn-primary" onClick={() => onNavigate('dashboard')}>
            Back to Dashboard
          </button>
        </div>
      ) : (
        <div className="recovery-transactions">
          {transactions.map((tx) => (
            <div key={tx.transaction_id} className="recovery-card">
              <div className="recovery-card-header">
                <h3>Transaction {tx.transaction_id.slice(0, 8)}...</h3>
                <span className={`state-badge state-${tx.current_state}`}>
                  {formatState(tx.current_state)}
                </span>
              </div>

              <div className="recovery-card-details">
                <div className="recovery-evidence">
                  <h4>Evidence</h4>
                  <ul>
                    <li className={tx.staging_exists ? 'present' : 'missing'}>
                      Staging: {tx.staging_exists ? 'Present' : 'Missing'}
                    </li>
                    <li className={tx.target_exists ? 'present' : 'missing'}>
                      Target: {tx.target_exists ? 'Present' : 'Missing'}
                    </li>
                    <li className={tx.manifest_exists ? 'present' : 'missing'}>
                      Manifest: {tx.manifest_exists ? 'Present' : 'Missing'}
                    </li>
                  </ul>
                </div>

                <div className="recovery-diagnostics">
                  <h4>Diagnostics</h4>
                  <ul>
                    {tx.diagnostics.map((d, i) => (
                      <li key={i}>{d}</li>
                    ))}
                  </ul>
                </div>
              </div>

              <div className="recovery-actions">
                <button
                  className="btn-primary"
                  onClick={() => handleRecover(tx.transaction_id)}
                  disabled={recovering === tx.transaction_id}
                >
                  {recovering === tx.transaction_id ? 'Recovering...' : 'Recover'}
                </button>
                {tx.target_path && (
                  <button className="btn-secondary" onClick={() => {/* open dir */}}>
                    Open Target
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
