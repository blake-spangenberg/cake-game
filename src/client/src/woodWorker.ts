/** Paints each `WoodLook` it is sent and sends back its texels, for `paintTableWood` in `props.ts`. */
import { type WoodLook, paintWood } from "./woodPaint";

addEventListener("message", (event: MessageEvent<WoodLook>) => {
  const texels = paintWood(event.data);
  postMessage(texels, { transfer: [texels.buffer] });
});
