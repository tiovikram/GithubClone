// Icons used only by the packages area (same 16x16 stroke style as components/icons.tsx).
import type { SVGProps } from "react";

type P = SVGProps<SVGSVGElement> & { size?: number };

function Svg({ size = 16, children, ...rest }: P) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5}
      strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="icon" {...rest}>
      {children}
    </svg>
  );
}

export const PackageIcon = (p: P) => (
  <Svg {...p}><path d="M8 1.5 14 4.75v6.5L8 14.5 2 11.25v-6.5Z" /><path d="M2 4.75 8 8l6-3.25M8 8v6.5" /></Svg>
);
export const AppsIcon = (p: P) => (
  <Svg {...p}><rect x="2" y="2" width="5" height="5" rx="1" /><rect x="9" y="2" width="5" height="5" rx="1" /><rect x="2" y="9" width="5" height="5" rx="1" /><rect x="9" y="9" width="5" height="5" rx="1" /></Svg>
);
export const ShieldIcon = (p: P) => (
  <Svg {...p}><path d="M8 1.5 13.5 3.5v4c0 3.2-2.3 5.8-5.5 7-3.2-1.2-5.5-3.8-5.5-7v-4Z" /></Svg>
);
