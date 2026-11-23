# Design
The game design the owner sets. Build every change players would notice to this file, and record new design decisions here.

## Pitch
A competitive game a Zoom room of about a dozen people can start and finish in about a minute and enjoy several days in a row. Players position a 3D cake and cut it in half with a knife, and are judged by how evenly their cut split the cake.

The game is intuitive, since it depicts a scenario that could exist in the real world, and the resulting rank is definitive and indisputable.

## Access
- The game runs in a browser tab opened from a link posted in the Zoom chat. No installs, accounts, or controllers.
- It is not a Zoom App: publishing one for guests outside the host's Zoom account takes Zoom's Marketplace review (4-12 weeks), and guest mode only reaches invited, unauthenticated participants. The web build stays runnable inside Zoom's embedded browser (WebView2) so a Zoom App stays possible later.
- Players join late and near instantly: a late joiner spectates the round in progress and plays the next one.
- Nothing carries over between rounds: no leaderboard history, no stored player identity.

## Lobby
- The first player to open the link is the host. If the host leaves, the role passes to another player.
- Every player types a name before joining. This is the one place the game requires the keyboard.
- The host picks the round's cake from a picker preset to Random, then presses Start.
- A "How to play" card stays visible so players learn while the lobby fills:
    - Click and drag the cake onto the cake stand
    - Adjust the rotation of the cake stand and the angle of the knife to line up the perfect cut
    - Press Cut! to lock in, cutting your cake in half
    - The winner is the player whose halves are most similar by weight
- A credits page, reachable from the lobby, says the game is inspired by the German game show Schlag den Henssler, lists the license credits for every sourced asset, and names the author, Blake Spangenberg.
- After the results, everyone returns to the lobby for the next round.

## Fairness
- All players get the same time, cake, knife, and starting positions. The cut is deterministic given the cake's and knife's poses.
- Each player has 30 seconds to cut. When time runs out, the game presses `Cut!` for the player, and that cut is judged like any other.
- The countdown is a pie that slowly loses area. A red numeric timer appears only in the last 10 seconds.
- Controls and measurement are precise enough that there are no ties. Only identical cuts tie, and the earlier cut wins. Players who never touched the controls share their rank.

## Controls
- Mouse only: no keyboard input is required during play. Dragging anywhere else swings the camera around the table's center and the wheel zooms. Arrow keys and WASD also swing it, as a second-class option.
- Prefer diegetic information. Words, numbers, and instructions are a last resort.
- The knife is a mezzaluna whose cut plane always passes through a fixed point above the cutting region, a cake stand the player sets the cake on. Dragging the knife (blade or either handle) tilts it with the mouse's left/right travel and lowers it toward the cake with up/down travel to check the line. Lowering stops at the cake's surface without pushing it, eases back up when the player lets go, and never cuts. The knife never turns: the player turns the cake stand instead.
- Every control moves relative to where its drag started, eased through springs.
- A big, obvious `Cut!` button, clearly separated from the other controls, freezes time, physics, and input and cuts at once along the knife's current pose, so a cake still settling is cut as it is at that instant.
- Tilt is limited to roughly 45-60 degrees from vertical, so every cut still looks like slicing a cake.
- The cake moves one way only: the player picks it up by clicking a point on its surface, and it hangs from that point under gravity. Grabbing a tall cake near its bottom and lifting it flips it inverted in the hand, and it lands that way when lowered. Letting go hands the cake to gravity and table collision in full, and it settles at its natural resting pose. Where the cake is set down decides where the cut plane lands and how it was grabbed decides its orientation, so knife and cake together reach every plane.
- Placement plays out in stages, each gentle enough to keep what the last one set and each punished by physics when rushed:
    - A carried cake hovers a centimeter above what lies beneath it, as a stand pickup does, rising smoothly toward the stand's height as it nears across most of the table, and where it is grabbed sets its orientation
    - Dragging it onto the stand keeps that orientation, unless done too violently or caught on the rim, which may turn it
    - Picking it up while on the stand lifts it only enough to slide it across the plate, keeping its orientation
    - Turning the stand adjusts it further. Circling the mouse around the stand turns it the same way for as long as the circling goes on, and it stops when the circling stops
    - Tilting the knife makes the final adjustment. A lowered knife stops both tilting and turning where it meets the cake or stand rather than jumping clear
- The perspective must not mislead: a cut must never be unexpectedly bad because of an illusion of perspective. Players check the knife against the cake from other camera angles and by lowering the knife. Knife and cake cast shadows on the table and on each other as depth cues.
- A cut is final.
- The game relies on player entropy to widen the spread of outcomes. The knife and cake start on separate sides of the table, the knife at an unhelpful orientation. The more input distance a cut requires, the more a skilled player can express their advantage.

## Opponents
- The left side of the screen is the player's own game, the right side a view of every opponent.
- Each opponent appears as their cake stand, cake, and knife floating on a neutral background, without their table or camera, all from one shared fixed angle. The view is full fidelity and real time: copying a rival's line is part of the game.
- Each opponent carries a name label and gets a green check mark once they have cut.
- No one sees any score, their own included, until every player has cut or time runs out.

## Judging
The reveal is dramatic, with pomp and circumstance, and is worth a round running past a minute.
- The knife cuts at once, but the halves stay together until the reveal parts them.
- Once play ends, every player sees the same view for the rest of the round. The opponent views give way to a leaderboard that starts empty.
- Players are judged one at a time in random order. The camera cuts to each player's stand, labels their name at the top middle of the view, and orbits slowly clockwise throughout judging.
- Each scene opens with the knife plunged and the halves still together. The halves slowly slide apart in place, and a gram counter over each half counts up from 0 as they part. Both counters climb at the same rate, so a perfect cut stops both at the same instant and a bad cut keeps counting the larger half alone. The count starts fast and slows the closer it gets to half the cake's weight, so the last few grams tick slowly.
- Once the counting ends, the percentage score is revealed and the player joins the leaderboard at their rank. The first player judged takes 1st place until dethroned.
- Scores show as a percentage to the nearest 0.1%. The unrounded score breaks ties.
- Weight is volume: every material has the same density, so grams are volume times a constant and a cut can be judged by eye.

## Cakes
- Every round uses one of several cakes.
- Cakes are sourced 3D models rather than generated ones, since they are the star of the show and a generated asset dulls the impression. Models must be free (CC0 or CC-BY), with credits shown in game. The owner picks each model from a shortlist, and decides the set's size at pick time.
- The set shares one look: smooth stylized, mid-poly models, never low-poly or flat-shaded. If too few stylized models qualify, the whole set switches to photoreal scans rather than mixing the two, so a shortlist carries scan backups for the owner to pivot to.
- Every cake is a whole object. Nothing already sliced, bitten, or cut, real cakes included: the first cut the player sees is their own.
- A model keeps its real counterpart's asymmetry: one left or right shoe, never a mirrored one.
- Sessions may recolor a model, keep one item of a pair, and trim a thin part a cake could not hold. Any reshaping (bending, adding a knot) needs the owner's approval.
- Cakes are not limited to traditional looking ones, which are mostly symmetric. Steer away from simple geometric shapes and symmetric designs. Asymmetry comes from:
    - cakes that are asymmetric as sourced
    - models that were never meant to be cakes (e.g. a rubber duck with a scarf), whose cake nature shows only when the cut exposes the layers inside
        - Any object works (a rubber duck, an eggplant, a car) as long as a cake decorating expert could plausibly bake and carve a cake in that shape. Reject shapes with thin or floating parts a real cake could not hold
        - Cakes must be asymmetric enough that centering the cake and zeroing the knife is not the whole game: a round cake or any centrally symmetric shape is too easy. A mirror-symmetric shape (a car, a plain duck) is too easy as well, since cutting along its mirror plane halves it exactly, unless no pose it can rest in brings that plane within the knife's tilt limit. Prefer lopsided, irregular shapes, and favor the owner's ideas: one sneaker, an L-shaped sectional couch, a potato or eggplant, an oven mitt, a chicken nugget or tender, a wooden log, a lucky cat (maneki-neko), a yellow rain boot
- Cut faces show the cake inside, as sponge layers separated by frosting, since the secret-cake reveal depends on it.
- Every cake fits within the cutting region, and the blade spans the whole region at every tilt and heading, so no cake limits which cuts can reach it. Mind the knife-to-cake size ratio across the whole set.

## World, look, and sound
- The table floats in an abstract space with no floor or room around it. A cake dropped off it falls for about 3 seconds before it respawns.
- The knife's metal is shiny and the table shows wood grain.
- After the cut, the knife stays standing in the parted halves.
- A chop sound plays for the player's own cut and for opponents' cuts.

## Stretch goals
- Realistic cake layer texture on the cut faces
- Setting the cake on the table in any orientation other than its serving orientation leaves frosting smudges. Smudges are cosmetic and never change the weighed halves.
- The cursor is a hand that roughly animates open and closed as it grabs the cake and knife

## Constraints
- Prefer a self-hosted multiplayer server, and generally minimize vendor reliance. The server runs on the developer's home machine and is reachable over HTTPS by guests outside the company.
- The game must be performant. Prefer performance over saving power, but not for gains no player would notice.
