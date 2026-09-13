export function SummaryTile({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint: string;
}) {
  return (
    <div className="surface-panel p-4">
      <p className="text-[10px] uppercase tracking-wide text-text-muted">
        {label}
      </p>
      <p
        className="mt-1 truncate text-sm font-semibold text-text-primary"
        title={value}
      >
        {value}
      </p>
      <p className="mt-1 text-[11px] text-text-muted">{hint}</p>
    </div>
  );
}
