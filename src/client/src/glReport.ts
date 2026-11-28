import { IS_DEBUG_PAGE } from "./debug";

/** How long after a context is made its GL errors are polled, once a second. */
const ERROR_POLL_S = 10;
/** The most characters one line keeps, enough for three.js's shader error with its log. */
const MAX_LINE_LEN = 2000;

/**
 * A `?debug` page's report of what its WebGL contexts met, shown on top of the page: each
 * context's GPU and granted attributes, lost and restored contexts, GL errors, and every error and
 * warning the page logs, where three.js reports a shader that fails to compile. It is for a device
 * whose 3D view stays blank, so it shows on a touch device always and elsewhere only once
 * something went wrong.
 */
class GlReport {
  private readonly readout = document.createElement("pre");
  private readonly isTouch = matchMedia("(pointer: coarse)").matches;

  constructor() {
    this.readout.id = "gl-report";
    this.readout.hidden = !this.isTouch;
    document.body.append(this.readout);
    window.addEventListener("webglcontextlost", (event) => this.addProblem(`context lost: ${describeTarget(event)}`), true);
    window.addEventListener("webglcontextrestored", (event) => this.addProblem(`context restored: ${describeTarget(event)}`), true);
    window.addEventListener("error", (event) => this.addProblem(`error: ${event.message}`));
    window.addEventListener("unhandledrejection", (event) => this.addProblem(`unhandled: ${String(event.reason)}`));
    for (const level of ["error", "warn"] as const) {
      const log = console[level].bind(console);
      console[level] = (...args: unknown[]) => {
        log(...args);
        this.addProblem(`${level}: ${args.map(String).join(" ")}`);
      };
    }
  }

  /** Reports the GPU and the attributes `gl`, the context of the canvas `name`, was granted, then polls its GL errors. */
  addContext(name: string, gl: WebGLRenderingContext | WebGL2RenderingContext): void {
    const rendererInfo = gl.getExtension("WEBGL_debug_renderer_info");
    const gpu: unknown = gl.getParameter(rendererInfo ? rendererInfo.UNMASKED_RENDERER_WEBGL : gl.RENDERER);
    const attributes = gl.getContextAttributes();
    const maxTexture: unknown = gl.getParameter(gl.MAX_TEXTURE_SIZE);
    this.addLine(
      `${name}: ${String(gpu)}, antialias ${attributes?.antialias}, stencil ${attributes?.stencil}, max texture ${String(maxTexture)}`,
    );
    let pollsLeft = ERROR_POLL_S;
    const poll = setInterval(() => {
      const error = gl.getError();
      if (error !== gl.NO_ERROR) this.addProblem(`${name}: GL error 0x${error.toString(16)}`);
      if (--pollsLeft === 0) clearInterval(poll);
    }, 1000);
  }

  private addProblem(line: string): void {
    this.readout.hidden = false;
    this.addLine(line);
  }

  private addLine(line: string): void {
    this.readout.textContent += `${line.slice(0, MAX_LINE_LEN)}\n`;
  }
}

function describeTarget(event: Event): string {
  return event.target instanceof HTMLElement ? event.target.id || event.target.className || event.target.tagName : "unknown";
}

export const glReport = IS_DEBUG_PAGE ? new GlReport() : null;
