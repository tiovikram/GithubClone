// Small hand-drawn 16x16 icon set (Primer/Octicon-inspired shapes).
import type { ReactNode, SVGProps } from "react";

type P = SVGProps<SVGSVGElement> & { size?: number };

function Svg({ size = 16, children, ...rest }: P & { children: ReactNode }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5}
      strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="icon" {...rest}>
      {children}
    </svg>
  );
}

export const RepoIcon = (p: P) => (
  <Svg {...p}><path d="M3 13.5V3a1.5 1.5 0 0 1 1.5-1.5H13v10H4.5A1.5 1.5 0 0 0 3 13a1.5 1.5 0 0 0 1.5 1.5H6" /><path d="M8 13v2.5l1-.8 1 .8V13" /></Svg>
);
export const LockIcon = (p: P) => (
  <Svg {...p}><rect x="3" y="7" width="10" height="7.5" rx="1.5" /><path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" /></Svg>
);
export const IssueOpenIcon = (p: P) => (
  <Svg {...p}><circle cx="8" cy="8" r="6.25" /><circle cx="8" cy="8" r="1" fill="currentColor" /></Svg>
);
export const IssueClosedIcon = (p: P) => (
  <Svg {...p}><circle cx="8" cy="8" r="6.25" /><path d="M5.5 8.2 7.2 9.8 10.6 6.3" /></Svg>
);
export const PullIcon = (p: P) => (
  <Svg {...p}><circle cx="4" cy="3.5" r="1.6" /><circle cx="4" cy="12.5" r="1.6" /><circle cx="12" cy="12.5" r="1.6" /><path d="M4 5.1v5.8M12 10.9V6a2 2 0 0 0-2-2H7.5M9 2.5 7.5 4 9 5.5" /></Svg>
);
export const MergeIcon = (p: P) => (
  <Svg {...p}><circle cx="4" cy="3.5" r="1.6" /><circle cx="4" cy="12.5" r="1.6" /><circle cx="12" cy="9" r="1.6" /><path d="M4 5.1v5.8M4.6 5c1 2.4 3.2 4 5.8 4" /></Svg>
);
export const PullClosedIcon = (p: P) => (
  <Svg {...p}><circle cx="4" cy="3.5" r="1.6" /><circle cx="4" cy="12.5" r="1.6" /><circle cx="12" cy="12.5" r="1.6" /><path d="M4 5.1v5.8M12 10.9V8M10.3 2.3l3.4 3.4M13.7 2.3l-3.4 3.4" /></Svg>
);
export const FileIcon = (p: P) => (
  <Svg {...p}><path d="M9.5 1.5H4A1.5 1.5 0 0 0 2.5 3v10A1.5 1.5 0 0 0 4 14.5h8a1.5 1.5 0 0 0 1.5-1.5V5.5z" /><path d="M9.5 1.5v4h4" /></Svg>
);
export const DirIcon = (p: P) => (
  <svg width={p.size ?? 16} height={p.size ?? 16} viewBox="0 0 16 16" aria-hidden="true" className="icon icon-dir">
    <path fill="currentColor" d="M1.75 2.5h4.1c.4 0 .78.18 1.03.5l.9 1.1c.1.12.24.19.4.19h6.07c.69 0 1.25.56 1.25 1.25v7.71c0 .69-.56 1.25-1.25 1.25H1.75C1.06 14.5.5 13.94.5 13.25V3.75c0-.69.56-1.25 1.25-1.25z" />
  </svg>
);
export const CommitIcon = (p: P) => (
  <Svg {...p}><circle cx="8" cy="8" r="2.5" /><path d="M1 8h4.5M10.5 8H15" /></Svg>
);
export const BranchIcon = (p: P) => (
  <Svg {...p}><circle cx="4.5" cy="3.5" r="1.6" /><circle cx="4.5" cy="12.5" r="1.6" /><circle cx="11.5" cy="4.5" r="1.6" /><path d="M4.5 5.1v5.8M11.5 6.1c0 3-7 2-7 4.8" /></Svg>
);
export const GearIcon = (p: P) => (
  <Svg {...p}><circle cx="8" cy="8" r="2.2" /><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4" /></Svg>
);
export const CodeIcon = (p: P) => (
  <Svg {...p}><path d="M5.5 4 1.5 8l4 4M10.5 4l4 4-4 4" /></Svg>
);
export const BookIcon = (p: P) => (
  <Svg {...p}><path d="M1.5 2.5h4A2.5 2.5 0 0 1 8 5v9a2 2 0 0 0-2-2H1.5zM14.5 2.5h-4A2.5 2.5 0 0 0 8 5v9a2 2 0 0 1 2-2h4.5z" /></Svg>
);
export const CopyIcon = (p: P) => (
  <Svg {...p}><rect x="5.5" y="5.5" width="9" height="9" rx="1.5" /><path d="M10.5 5.5V3A1.5 1.5 0 0 0 9 1.5H3A1.5 1.5 0 0 0 1.5 3v6A1.5 1.5 0 0 0 3 10.5h2.5" /></Svg>
);
export const CheckIcon = (p: P) => (
  <Svg {...p}><path d="m3 8.5 3 3 7-7" /></Svg>
);
export const KeyIcon = (p: P) => (
  <Svg {...p}><circle cx="5" cy="11" r="3.2" /><path d="m7.3 8.7 6.2-6.2M11.5 4.5l2 2M10 6l1.5 1.5" /></Svg>
);
export const PlusIcon = (p: P) => (
  <Svg {...p}><path d="M8 2.5v11M2.5 8h11" /></Svg>
);
export const ChevronDownIcon = (p: P) => (
  <Svg {...p}><path d="m4 6 4 4 4-4" /></Svg>
);
export const CommentIcon = (p: P) => (
  <Svg {...p}><path d="M2.5 3h11a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1H8l-3 2.5V12H2.5a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z" /></Svg>
);
export const StarIcon = (p: P) => (
  <Svg {...p}><path d="m8 1.8 1.9 3.9 4.3.6-3.1 3 .7 4.3L8 11.6l-3.8 2 .7-4.3-3.1-3 4.3-.6z" /></Svg>
);
export const ForkIcon = (p: P) => (
  <Svg {...p}><circle cx="4" cy="3" r="1.5" /><circle cx="12" cy="3" r="1.5" /><circle cx="8" cy="13" r="1.5" /><path d="M4 4.5v1A2 2 0 0 0 6 7.5h4a2 2 0 0 0 2-2v-1M8 7.5v4" /></Svg>
);
export const HistoryIcon = (p: P) => (
  <Svg {...p}><path d="M1.8 8a6.2 6.2 0 1 0 1.8-4.4M1.5 1.8v2.7h2.7" /><path d="M8 4.8V8l2.2 1.6" /></Svg>
);
export const LogoIcon = ({ size = 32 }: { size?: number }) => (
  <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true" className="logo">
    <rect width="32" height="32" rx="7" fill="currentColor" />
    <path d="M9 23V9h13v3.2h-9.2v2.6h7.8V18h-7.8v5z" fill="var(--bg-header)" />
  </svg>
);
