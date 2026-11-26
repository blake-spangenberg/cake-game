/** How a player's place and score read on the leaderboards and the standup order. */
import type { Standing } from "./net";

/** Places that carry a medal, gold, silver, then bronze. */
const MEDAL_PLACES = 3;

/** Whether `standing`'s knife missed the cake, which the core scores as the whole cake above the knife, as `src/server`'s `is_miss` judges it. */
export function isMiss(standing: Standing): boolean {
  return standing.below_m3 <= 0;
}

/** A score as players see it, to the nearest 0.1%, or `Miss` for a knife that missed the cake. */
export function formatScore(scorePct: number, isMissed: boolean): string {
  return isMissed ? "Miss" : `${scorePct.toFixed(1)}%`;
}

/** Shows `standing`'s score in `element`, red for a miss. */
export function setScoreText(element: HTMLElement, standing: Standing): void {
  const text = formatScore(standing.score, isMiss(standing));
  // Called every frame by the judging, so it touches the page only on a change.
  if (element.textContent !== text) element.textContent = text;
  element.classList.toggle("is-miss", isMiss(standing));
}

/** A place as English writes it: 1st, 2nd, 3rd, 4th, 11th, 21st. */
export function formatPlace(place: number): string {
  const teen = place % 100 >= 11 && place % 100 <= 13;
  const suffix = teen ? "th" : (["th", "st", "nd", "rd"][place % 10] ?? "th");
  return `${place}${suffix}`;
}

/** Whether a list shows a medal at `place`, as it does for the top three; none for no place, a miss's. */
export function isMedalPlace(place: number | null): boolean {
  return place !== null && place <= MEDAL_PLACES;
}

/** The medal slot beside a name: gold, silver, or bronze for the top three places, else empty, so names line up. */
export function buildMedal(place: number | null): HTMLSpanElement {
  const medal = buildSpan("medal", "");
  if (isMedalPlace(place)) medal.classList.add(`medal-${place}`);
  return medal;
}

/** The place `standing` shows on a list at `place`: none for a miss, so a miss never reads as a podium finish or wears a medal. */
export function findShownPlace(standing: Standing, place: number): number | null {
  return isMiss(standing) ? null : place;
}

/** A leaderboard row: the place, medal, name, and score of `standing`, at `place`. */
export function buildStandingRow(standing: Standing, place: number): HTMLLIElement {
  const item = document.createElement("li");
  const score = buildSpan("standing-score", "");
  setScoreText(score, standing);
  const shownPlace = findShownPlace(standing, place);
  const placeText = shownPlace === null ? "" : formatPlace(shownPlace);
  item.append(buildSpan("standing-rank", placeText), buildMedal(shownPlace), buildSpan("member-name", standing.name), score);
  return item;
}

export function buildSpan(className: string, text: string): HTMLSpanElement {
  return Object.assign(document.createElement("span"), { className, textContent: text });
}
