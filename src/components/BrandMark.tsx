import type { SVGProps } from "react";
import { BRAND_PATHS } from "./brand-paths";

/** Brand artwork shares the Pages SVG master; inherited tokens follow the app theme. */
export function BrandMark(props: SVGProps<SVGSVGElement>) {
  return (
    <svg
      width="32"
      height="32"
      viewBox="0 0 256 256"
      aria-hidden="true"
      focusable="false"
      {...props}
    >
      {BRAND_PATHS.map((path) => (
        <path key={path.d} {...path} />
      ))}
    </svg>
  );
}
