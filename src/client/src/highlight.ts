import type * as THREE from "three";

/** How much a highlighted dark surface brightens everywhere, as a share of its lit color, so its shadows and texture keep their contrast. */
const GAIN = 0.45;
/** The glow added to a dark surface toward its silhouette, in linear light at a grazing view. */
const RIM = 0.17;
/** How tightly the glow and the edge shading hug the silhouette: higher keeps more of the surface untouched. */
const RIM_POWER = 3;
/** A warm white, so the glow reads as light on the object rather than a colored overlay. */
const RIM_COLOR = "vec3(1.0, 0.95, 0.86)";
/**
 * The linear luminance range over which a pixel turns from lifted to darkened: a lift cannot show
 * on a surface near white, which the screen already shows at or near full brightness.
 */
const LIGHT_FROM = 0.2;
const LIGHT_TO = 0.5;
/** What a light surface's linear color is multiplied by, alike in every channel, so it darkens and keeps its hue. */
const SHADE = 0.76;
/** How much a light surface darkens toward its silhouette at a grazing view, which outlines it softly without an outline pass. */
const EDGE_SHADE = 0.45;
/** Rec. 709 luminance weights, for linear color. */
const LUMINANCE = "vec3(0.2126, 0.7152, 0.0722)";

/** How strongly a material from `addHighlight` shows its highlight, from 0 (none) to 1. */
export interface Highlight {
  value: number;
}

/**
 * Gives `material` a faint highlight whose strength the returned uniform sets, judged per pixel
 * by how bright the surface is lit there: a dark surface brightens slightly and glows toward its
 * silhouette, and a light one, where a lift cannot show, darkens, more toward its silhouette.
 * Changing the strength recompiles nothing, so a material shows it at no cost.
 */
export function addHighlight(material: THREE.MeshStandardMaterial): Highlight {
  const highlight: Highlight = { value: 0 };
  material.onBeforeCompile = (shader) => {
    shader.uniforms.highlightShare = highlight;
    shader.fragmentShader = `uniform float highlightShare;\n${shader.fragmentShader}`.replace(
      "#include <opaque_fragment>",
      `float highlightRim = pow(1.0 - saturate(dot(geometryNormal, geometryViewDir)), ${RIM_POWER.toFixed(2)});
      float highlightLightShare = smoothstep(${LIGHT_FROM.toFixed(2)}, ${LIGHT_TO.toFixed(2)}, dot(outgoingLight, ${LUMINANCE}));
      vec3 highlightLifted = outgoingLight * (1.0 + ${GAIN.toFixed(2)}) + ${RIM.toFixed(2)} * highlightRim * ${RIM_COLOR};
      vec3 highlightShaded = min(outgoingLight, vec3(1.0)) * ${SHADE.toFixed(2)} * (1.0 - ${EDGE_SHADE.toFixed(2)} * highlightRim);
      outgoingLight = mix(outgoingLight, mix(highlightLifted, highlightShaded, highlightLightShare), highlightShare);
      #include <opaque_fragment>`,
    );
  };
  return highlight;
}
