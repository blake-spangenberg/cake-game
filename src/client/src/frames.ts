/**
 * Resolves just after the `count`th next frame is drawn, so work awaiting it lands between frames
 * rather than stacking onto one. A hidden page draws no frames, so there it resolves without them,
 * and loading a round in a background tab still finishes in time.
 */
export async function waitFrames(count: number): Promise<void> {
  for (let i = 0; i < count; i += 1) await waitFrame();
}

function waitFrame(): Promise<void> {
  return new Promise((resolve) => {
    if (document.hidden) {
      resolve();
      return;
    }
    const onVisibilityChange = () => resolve();
    document.addEventListener("visibilitychange", onVisibilityChange, { once: true });
    requestAnimationFrame(() => {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      setTimeout(resolve, 0);
    });
  });
}
