/**
 * QR codes both ways: drawing one to hand a server to a phone, and reading one to add a server.
 *
 * ## Drawing
 *
 * The encoding comes from `uqr` — zero dependencies, pinned exactly — because the Reed-Solomon and
 * masking half of QR is a few hundred lines that are easy to get subtly wrong and impossible to
 * notice being wrong until a scanner refuses. The drawing is done here: `uqr` hands back the module
 * grid, and one path of unit squares is all a code needs. Its own SVG renderer would have to be
 * inserted as markup, which `dom.ts` allows only for strings this app composed.
 *
 * Always dark on a light ground, in both themes. Many scanners cannot read an inverted code, so
 * the dark-mode palette does not apply here.
 */

import { encode } from "uqr";

import { svg } from "../dom";

export function qrCode(text: string, size = 232): SVGElement {
  // Medium correction: a share link is long, and M keeps the code at a density a phone camera
  // reads off a laptop screen, while still surviving glare and a slightly cropped frame.
  const { data, size: modules } = encode(text, { ecc: "M", border: 2 });

  let path = "";
  for (let y = 0; y < modules; y++) {
    for (let x = 0; x < modules; x++) {
      if (data[y][x]) path += `M${x} ${y}h1v1h-1z`;
    }
  }

  return svg(
    "svg",
    {
      class: "qr",
      viewBox: `0 0 ${modules} ${modules}`,
      width: size,
      height: size,
      // Anti-aliased module edges blur into their neighbours at small sizes.
      "shape-rendering": "crispEdges",
      role: "img",
      "aria-label": "QR code of the share link",
    },
    svg("rect", { width: modules, height: modules, fill: "#fff" }),
    svg("path", { d: path, fill: "#000" }),
  ) as SVGElement;
}

/**
 * Reads a QR code out of an image: a screenshot, a photo, a file someone was sent.
 *
 * An image rather than a camera. A desktop's camera faces the user, and holding a phone up to a
 * webcam is an awkward way to move a link that is usually already on this machine as a
 * screenshot. It also needs no camera permission, which WebKitGTK does not grant an app page.
 *
 * Decoding is `jsQR` (Apache-2.0, no dependencies, pinned exactly), for the same reason encoding is
 * not hand-written. Both colour schemes are tried, because a code screenshotted from a dark-mode
 * app is often light-on-dark.
 */
export async function readQrCode(image: Blob): Promise<string | null> {
  const { default: jsQR } = await import("jsqr");
  const bitmap = await createImageBitmap(image);
  const canvas = document.createElement("canvas");
  canvas.width = bitmap.width;
  canvas.height = bitmap.height;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return null;
  ctx.drawImage(bitmap, 0, 0);
  bitmap.close();

  const pixels = ctx.getImageData(0, 0, canvas.width, canvas.height);
  return jsQR(pixels.data, pixels.width, pixels.height, { inversionAttempts: "attemptBoth" })?.data ?? null;
}
