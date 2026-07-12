interface ProgressProps {
  label: string;
  value?: number;
  max?: number;
  detail?: string;
}

export function Progress({ label, value, max = 100, detail }: ProgressProps) {
  const determinate = typeof value === 'number' && max > 0;
  const percent = determinate ? Math.min(100, Math.max(0, (value / max) * 100)) : 0;

  return (
    <div className="imagedb-progress">
      <div className="imagedb-progress__header">
        <span>{label}</span>
        <span className="imagedb-progress__value">
          {determinate ? `${Math.round(percent)}%` : '正在统计'}
        </span>
      </div>
      <div
        className={`imagedb-progress__track ${determinate ? '' : 'is-indeterminate'}`}
        role="progressbar"
        aria-label={label}
        aria-valuemin={determinate ? 0 : undefined}
        aria-valuemax={determinate ? max : undefined}
        aria-valuenow={determinate ? value : undefined}
      >
        <span
          className="imagedb-progress__bar"
          style={{ transform: determinate ? `scaleX(${percent / 100})` : undefined }}
        />
      </div>
      {detail && <p className="imagedb-progress__detail">{detail}</p>}
    </div>
  );
}
