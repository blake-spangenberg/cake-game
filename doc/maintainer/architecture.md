# Architecture
How the single-player game is put together, for changing it without breaking determinism or feel.

## Layout
| Path | Holds |
|---|---|
| `src/core/` | The deterministic simulation: physics world, hand, stand, knife, cut, and the input wire format. Everything the server must agree on lives here |
| `src/wasm/` | `wasm-bindgen` bindings exposing the core and its layout constants to the browser |
| `src/client/` | Vite + Three.js client: drawing, mouse and keyboard handling, springs, camera, cut animation |
| `src/core/examples/` | `replay` prints determinism checkpoints for `make test`. `settle` prints how the cake moves through a scripted carry, drop, and stand turn, and `drops` how far and how long the cake moves after drops from many grab points, for tuning physics |

## One tick
The client eases raw mouse input through springs in JavaScript, quantizes the eased values, and sends them as one `Input` per 60 Hz tick. Easing before quantizing keeps JS math out of the simulation while letting every control feel smooth. The core then, in order:
1. Steers the hand toward the target, lifting the cake's bounding box just clear of the surface beneath it, or lets go the moment the hand input stops
2. Turns the stand toward the input angle, speed-limited
3. Sets the knife's tilt and lowering, the lowering clamped where the curved edge meets the cake or the stand's plate
4. Slows a resting cake's spin relative to its support, then steps Rapier and respawns a cake that fell off the table

Every control is relative: a drag moves its object by how far the mouse travels from where it pressed, never toward the cursor, so no cursor path can flip an object.

## Knife and stand
The cut plane always passes through `PIVOT`. The knife has two axes, tilt about its fixed edge direction and lowering within its plane, and the stand turning the cake supplies the third, so knife and placement together still reach every plane. The mezzaluna's edge is a circular arc, so the lowering limit minimizes a convex function per section segment rather than taking the segment's endpoints.

The knife collides only with a held cake. A free cake is never pushed by the blade, because the lowering limit already stops the edge at its surface, and a held cake is left out of that limit so the blade holds its place against it. Contacts alone cannot stop a held cake: the kinematic hand's rigid grip outmuscles any contact and squeezes the cake through the thin blade. So each hand move first sweeps the cake's shape against the knife and stops or slides it there, and the cake has CCD on so its swing cannot tunnel through either. Removing either one fails a core test.

## Physics
Rapier's slop, contact prediction, and sleep thresholds scale with `length_unit`, which is set to a decimeter because the defaults assume meter-sized objects.

A convex hull face with many vertices resting on the table jitters in Rapier and never sleeps, even in a bare scene, and no solver setting fixes it. So collider hulls are built coarser than the drawn and weighed mesh, which also gives a cake on its side facets to stop on.

Damping cannot stop an off-balance cake rocking or rolling on its rim, because gravity keeps driving it. A soft cake squashing at its contact acts as rolling resistance instead: a fixed loss of spin per second, relative to the table or stand beneath, applied only while touching. It stops small rolls outright yet lets a cake tip over, and being relative to the support keeps the turning stand carrying the cake.

The carried cake's height follows its current bounding box: its bottom clears the table, then rises to clear the stand as the box nears the stand's rim. Run the `settle` and `drops` examples before and after changing damping, friction, resistance, or speeds.

## Client
Performance beats power saving, but not for gains no player sees: the renderer asks for the high-performance GPU, and frames beyond the simulation's 60 Hz are not drawn. Development builds (`make run`) have no round time limit, and production builds use `ROUND_TICKS`. The cut animation is client-only: the simulation freezes and scores at the press, and the client then sweeps the knife down through the cake, rocks it, and parts the halves. Each half's cut face is a sponge plane drawn only where the half's surfaces, counted in the stencil buffer, show it is inside the cake, so it needs no cap geometry and works for any closed mesh.
