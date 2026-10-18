// Icons used only by the social area (same 16x16 stroke style as components/icons.tsx).
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

export const BellIcon = (p: P) => (
  <Svg {...p}><path d="M4 11V7a4 4 0 0 1 8 0v4l1.5 1.5h-11z" /><path d="M6.5 14a1.5 1.5 0 0 0 3 0" /></Svg>
);
export const EyeIcon = (p: P) => (
  <Svg {...p}><path d="M1 8s2.5-5 7-5 7 5 7 5-2.5 5-7 5-7-5-7-5z" /><circle cx="8" cy="8" r="2" /></Svg>
);
export const PeopleIcon = (p: P) => (
  <Svg {...p}><circle cx="5.5" cy="5" r="2.5" /><path d="M1 14a4.5 4.5 0 0 1 9 0" /><path d="M10.5 2.6a2.5 2.5 0 0 1 0 4.8M12 9.5a4.5 4.5 0 0 1 3 4.5" /></Svg>
);
export const PulseIcon = (p: P) => (
  <Svg {...p}><path d="M1 8h3l2-5 4 10 2-5h3" /></Svg>
);
export const GistIcon = (p: P) => (
  <Svg {...p}><rect x="1.5" y="1.5" width="13" height="13" rx="2" /><path d="m6 6-2 2 2 2M10 6l2 2-2 2" /></Svg>
);
export const BlockIcon = (p: P) => (
  <Svg {...p}><circle cx="8" cy="8" r="6.5" /><path d="m3.5 3.5 9 9" /></Svg>
);
export const MailIcon = (p: P) => (
  <Svg {...p}><rect x="1.5" y="3" width="13" height="10" rx="1.5" /><path d="m2 4 6 5 6-5" /></Svg>
);
export const LinkIcon = (p: P) => (
  <Svg {...p}><path d="M7 9a3 3 0 0 0 4.2 0l2-2A3 3 0 0 0 9 2.8l-1 1" /><path d="M9 7a3 3 0 0 0-4.2 0l-2 2A3 3 0 0 0 7 13.2l1-1" /></Svg>
);
export const LocationIcon = (p: P) => (
  <Svg {...p}><path d="M8 15s5-4.5 5-8.5a5 5 0 0 0-10 0C3 10.5 8 15 8 15z" /><circle cx="8" cy="6.5" r="1.8" /></Svg>
);
export const OrgIcon = (p: P) => (
  <Svg {...p}><path d="M2 14.5V3a1.5 1.5 0 0 1 1.5-1.5h5A1.5 1.5 0 0 1 10 3v11.5M10 7h2.5A1.5 1.5 0 0 1 14 8.5v6M1 14.5h14M5 5h2M5 8h2M5 11h2" /></Svg>
);
