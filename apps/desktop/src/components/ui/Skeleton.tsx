interface SkeletonProps {
  width?: string | number;
  height?: string | number;
  radius?: string | number;
  label?: string;
}

export function Skeleton({ width = '100%', height = 16, radius, label }: SkeletonProps) {
  return (
    <span
      className="imagedb-skeleton"
      style={{ width, height, borderRadius: radius }}
      role={label ? 'status' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    />
  );
}
