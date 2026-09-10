import { BRAND_PATHS } from "@/components/brand-paths";
import { isTauri } from "@/lib/platform";
import { invoke } from "@/lib/ipc";

/** Draw the same vector planes used by BrandMark, including resolved theme colors. */
export function drawBrandIcon(
  root: HTMLElement = document.documentElement,
): HTMLCanvasElement | null {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 256;
  const context = canvas.getContext("2d");
  if (!context) return null;
  const style = getComputedStyle(root);
  context.fillStyle = "#0e1026";
  context.beginPath();
  context.roundRect(16, 16, 224, 224, 49);
  context.fill();
  context.translate(29, 29);
  context.scale(0.75, 0.75);
  for (const path of BRAND_PATHS) {
    const [, token, fallback] = path.fill.match(
      /var\((--[\w-]+),\s*([^)]+)\)/,
    )!;
    context.fillStyle = style.getPropertyValue(token).trim() || fallback;
    context.fill(new Path2D(path.d));
  }
  return canvas;
}

/** Native Dock/taskbar and browser tab share the active application accent. */
export function watchBrandIcon(): () => void {
  let frame = 0;
  let previous = "";
  const update = () => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      const canvas = drawBrandIcon();
      if (!canvas) return;
      const url = canvas.toDataURL("image/png");
      if (url === previous) return;
      previous = url;
      const favicon =
        document.querySelector<HTMLLinkElement>('link[rel="icon"]');
      if (favicon) {
        favicon.type = "image/png";
        favicon.href = url;
      }
      if (isTauri()) {
        const bytes = Array.from(atob(url.split(",")[1]), (char) =>
          char.charCodeAt(0),
        );
        void invoke("set_brand_icon", { png: bytes }).catch((error) =>
          console.warn("Could not update the application icon", error),
        );
      }
    });
  };
  const observer = new MutationObserver(update);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class", "style"],
  });
  update();
  return () => {
    observer.disconnect();
    cancelAnimationFrame(frame);
  };
}
