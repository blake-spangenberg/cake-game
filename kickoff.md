# Kickoff

## Elevator Pitch

A fun competitive game that can be started and finished in about a minute given a Zoom room filled with a dozen people and enjoyed multiple days in a row. Players position and cut a 3D cake in half with a knife and are judged by how accurately their cut split the cake in half.

The game is intuitive (it depicts a scenario that could exist in the real world) and the resulting player rank is definitive and undisputable.

## For the concept to stick the landing

- The game runs in a browser tab opened from a link posted in the Zoom chat. No program installs, accounts, controllers, etc. Players must be able to join late and near instantaneously: a late joiner spectates the round in progress and plays the next one.
    - Not a Zoom App: publishing one for guests outside the host's Zoom account takes Zoom's Marketplace review (4-12 weeks), and guest mode only reaches invited, unauthenticated participants. Keep the web build able to run inside Zoom's embedded browser (WebView2) so a Zoom App stays possible later.
- There must be more than 1 cake, either selected randomly or chosen by the host
- The game must be fair. All players must have the same: time, cake, knife, starting positions. The cut must be deterministic given the cake/knife position/orientation.
- The time players have to make their cut must be limited to 30 seconds. When time runs out, the game presses `Cut!` for the player and that cut is judged like any other.
- The controls and the measurement must be precise so there are no ties. Only identical cuts can tie, and the earlier cut wins; players who never touched the controls share their rank.
- The controls must be simple, intuitive, and mouse only: no keyboard input is ever required. Dragging anywhere else swings the camera around the table's center and the wheel zooms; arrow keys and WASD also swing it, as a second-class option. Use words, numbers, instruction, etc. as a last result to communicate to the player, prefer diagetic information.
- The knife is a mezzaluna whose cut plane always passes through a fixed point above the cutting region, a cake stand the player sets the cake on. Dragging the knife (blade or either handle) tilts it with the mouse's left/right travel and lowers it toward the cake with up/down travel to check the line. Lowering stops at the cake's surface without pushing it, eases back up when the player lets go, and never cuts. The knife never turns: the player turns the cake stand instead. Every control moves relative to where the drag started, eased through springs. A big, obvious `Cut!` button, clearly separated from the other controls, freezes time, physics, and input and performs the cut immediately along the knife's current pose, so a cake still settling is cut as it is at that instant. Tilt is limited to roughly 45-60 degrees from vertical, so every cut still looks like slicing a cake.
- The cake moves one way only: the player picks it up by mouse click at a point on its surface and it hangs from that point under gravity. Grabbing a tall cake near its bottom and lifting it flips it inverted in the hand, and it lands in that orientation when lowered back to the table. Letting go hands the cake to gravity and table collision in full, and it settles at its natural resting pose. Where the cake is set down decides where the cut plane lands, and how it was grabbed decides its orientation, so knife and cake together reach every plane.
- Placement plays out in stages, each gentle enough to keep what the last one set and each punished by physics when rushed:
    - A carried cake hovers a centimeter above what lies beneath it, as a stand pickup does, rising smoothly toward the stand's height as it nears across most of the table, and where it is grabbed works its orientation
    - Dragging it onto the stand keeps that orientation, unless done too violently or caught on the rim, which may turn it
    - Picking it up while on the stand lifts it only enough to slide it across the plate, keeping its orientation
    - Turning the stand adjusts it further. Circling the mouse around the stand turns it the same way for as long as the circling goes on
    - Tilting the knife makes the final adjustment. A lowered knife stops both tilting and turning where it meets the cake or stand rather than jumping clear
- The perspective must not be ambigious. A cut must not be unexpectedly bad for a player due to the player's misunderstanding due to an illusion of perspective. Players can check the knife against the cake from alternate camera angles and by lowering the knife. Knife and cake cast shadows on the table and each other as natural depth cues.
- A cut must be final.
- Scores are a percentage shown to the nearest 0.1%. The unrounded score breaks ties.
- The game should rely on player entropy to increase variance of outcome. For example, do not start the knife over the cake but rather each on separate sides of the table. Start the knife at an unhelpful orientation. The more input distance that is required of players, the more a skilled player can express their advantage.
- Players must be able to see their opponents knife and cake movements in realtime, and clearly see when they have completed their cut. The view is full fidelity: copying a rival's line is part of the game.
- Players must not be able to see any player's score including their own until all players have completed the cut or time elapses.
- Result calulation must be dramatic (with pomp and circumstance). In multiplayer the knife cuts at once, but the halves stay together until the results sequence parts them. Cakes presented and judged one-by-one, from the worst cut to the best: each half placed on a balance scale and the grams tick up from 0 before the final score is revealed. The full reveal is worth a round running past a minute.
- Weight is volume: every material has the same density, so the scale's grams are volume times a constant and a cut can be judged by eye.
- Cakes must not be limited only to traditional looking cakes, especially since traditional cakes are most commonly symmetical. Steer from simple geometric shapes and symetrical designs. Asymmetry comes from:
    - cakes that are asymmetric as sourced
    - damage to a sourced cake: a missing slice, a bite, a collapsed tier
    - models that were never meant to be cakes (e.g. a rubber duck with a scarf), whose cake nature is revealed only when the cut exposes the layers inside
- Cut faces show cake inside, since the secret-cake reveal depends on it.
- The table floats in an abstract space with no floor or room around it. A cake dropped off it falls for about 3 seconds before it respawns.
- Nothing carries over between rounds: no leaderboard, no stored player identity.

## Stretch goals

- Realistic cake layer texture on the cut faces
- Setting the cake on the table in any orientation other than its serving orientation leaves frosting smudges. Smudges are cosmetic and never change the weighed halves.
- The player's cursor is a hand that roughly animates open and closed as it grabs the cake and knife

## Design considerations
- Prefer self-hosted multiplayer server, and generally minimize vendor reliance. The server runs on the developer's home machine and must be reachable over HTTPS by guests outside the company.
- Prefer to source existing 3D models of cakes rather than generate them. They are the star of the show and an agentic generated asset dulls the impression. Models must be free (CC0 or CC-BY), with credits shown in game.
- The game must be performant. Consider WebAssembly or other technology which has a heavier implementation weight but better long-term effects. Prefer performance over saving power, but not for gains no player would notice.
- Be mindful of the knife-to-cake size ratio individually and across the set of all cakes. Every cake must fit within the cutting region, and the blade must span the whole region at every tilt and heading, so no cake is limited in which cuts can reach it.

## Phasing

1. Clarify project principles and design requirements.

2. Define tech stack, toolchain, development enviornment.

3. Implement browser proof-of-concept

4. Player test group.
