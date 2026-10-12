import { BranchIcon } from "./icons";

export function BranchSelect({ branches, value, onChange, label = "Switch branch", id }: {
  branches: string[];
  value: string;
  onChange: (b: string) => void;
  label?: string;
  id?: string;
}) {
  const opts = branches.includes(value) || !value ? branches : [value, ...branches];
  return (
    <label className="branch-select">
      <BranchIcon />
      <span className="sr-only">{label}</span>
      <select id={id} value={value} onChange={(e) => onChange(e.target.value)} aria-label={label}>
        {opts.map((b) => (
          <option key={b} value={b}>
            {b}
          </option>
        ))}
      </select>
    </label>
  );
}
