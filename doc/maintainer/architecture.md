# Architecture
How the single-player game is put together, for changing it without breaking determinism or feel.

## Layout
| Path | Holds |
|---|---|
| `src/core/` | The deterministic simulation: physics world, hand, stand, knife, cut, and the input wire format. Everything the server must agree on lives here |
| `src/wasm/` | `wasm-bindgen` bindings exposing the core and its layout constants to the browser |
| `src/bake/` | `cake-bake`, a native tool that turns a repaired model into a `.cake` file with its collider hulls. See the asset pipeline in `tech-stack.md` |
| `tools/` | Setup and build helpers, and the model repair step `repair-model.py` with its tests |
| `src/client/` | Vite + Three.js client: drawing, mouse and keyboard handling, springs, camera, cut animation |
| `src/core/examples/` | `replay` prints determinism checkpoints for `make test`. `settle` prints how the cake moves through a scripted carry, drop, and stand turn, and `drops` how far and how long the cake moves after drops from many grab points, for tuning physics. Both take a `.cake` file as an argument in place of the placeholder |

## One tick
The client eases raw mouse input through springs in JavaScript, quantizes the eased values, and sends them as one `Input` per 60 Hz tick. Easing before quantizing keeps JS math out of the simulation while letting every control feel smooth. The core then, in order:
1. Steers the hand toward the target, lifting the cake's bounding box just clear of the surface beneath it, or lets go the moment the hand input stops
2. Turns the stand toward the input angle, speed-limited
3. Sets the knife's tilt and lowering, the lowering clamped where the curved edge meets the cake or the stand's plate
4. Slows a resting cake's spin relative to its support, then steps Rapier and respawns a cake that has been falling off the table for a few seconds

Every control is relative: a drag moves its object by how far the mouse travels from where it pressed, never toward the cursor, so no cursor path can flip an object. Springs only delay a drag's effect: a fast drag ends where a slow one of the same travel does, except on the stand, whose input is held within half a radian of the stand's actual angle so a fast circling drag cannot pile up turning the speed-limited stand would work off after release. The only clamp on a drag's target is a hard stop, where the core stopped the object short: the client sees it from the knife's tilt or lowering differing from what it sent, and from the stand's blocked flag, since the stand's speed limit already makes it lag its input.

## Knife and stand
The cut plane always passes through `PIVOT`. The knife has two axes, tilt about its fixed edge direction and lowering within its plane, and the stand turning the cake supplies the third, so knife and placement together still reach every plane. The mezzaluna's edge is a circular arc, so the lowering limit minimizes a convex function per section segment rather than taking the segment's endpoints.

A lowered knife blocks turns rather than rising out of their way: tilting stops where the edge would enter a solid, and so does turning the stand under a cake resting on it, which also stops the cake's spin. Both sweeps stop a few milliradians short of contact. At contact a cake settling a hair would cross the blade's plane, where the lowering limit jumps from a wall's foot to its top, and the knife would leap up after all.

The knife collides only with a held cake. A free cake is never pushed by the blade, because the lowering limit already stops the edge at its surface, and a held cake is left out of that limit so the blade holds its place against it. Contacts alone cannot stop a held cake: the kinematic hand's rigid grip outmuscles any contact and squeezes the cake through the thin blade. So each hand move first sweeps the cake's shape against the knife, stand, and table and stops or slides it there, which also keeps a carried cake from clipping into the stand, and the cake has CCD on so its swing cannot tunnel through either. Removing either one fails a core test.

## Physics
Rapier's slop, contact prediction, and sleep thresholds scale with `length_unit`, which is set to a decimeter because the defaults assume meter-sized objects.

A convex hull face with many vertices resting on the table jitters in Rapier and never sleeps, even in a bare scene, and no solver setting fixes it. So collider hulls are built coarser than the drawn and weighed mesh, which also gives a cake on its side facets to stop on.

Damping cannot stop an off-balance cake rocking or rolling on its rim, because gravity keeps driving it. A soft cake squashing at its contact acts as rolling resistance instead: a fixed loss of spin per second, relative to the table or stand beneath, applied only while touching. It stops small rolls outright yet lets a cake tip over, and being relative to the support keeps the turning stand carrying the cake.

A carried cake's lowest point clears the surface beneath it by a centimeter, the height a stand pickup lifts it. That surface eases from the table up to the stand's top along an S curve across most of the table, from half a meter out to the stand's rim, so a cake hanging low can still catch the rim and turn as it is dragged on. A cake grabbed while resting on the stand instead lifts only a centimeter above the grab point, so the plate still bears its far side and a gentle slide keeps its orientation, until its box leaves the stand and the usual carry takes over. Run the `settle` and `drops` examples before and after changing damping, friction, resistance, or speeds.

## Client
Performance beats power saving, but not for gains no player sees: the renderer asks for the high-performance GPU, and frames beyond the simulation's 60 Hz are not drawn. Development builds (`make run`) have no round time limit, and production builds use `ROUND_TICKS`. The cut animation is client-only: the simulation freezes and scores at the press, and the client then sweeps the knife down through the cake, with the halves already apart by just the blade's thickness so the blade shows in the cut, rocks it with its curved edge rolling without slipping, lets it swing a little past center and settle, and parts the halves. Each half's cut face is a sponge plane drawn only where the half's surfaces, counted in the stencil buffer, show it is inside the cake, so it needs no cap geometry and works for any closed mesh. The sponge plane is centered on the cake rather than the pivot, since a cake cut away from the stand has a section far from the pivot.
