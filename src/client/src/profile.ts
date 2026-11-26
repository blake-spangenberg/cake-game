import type { Profile } from "./net";

/**
 * The player's name, standup order preference, and whether they have a scrum update, remembered on
 * this browser, with their seat's token and when they last used the site, in any of its tabs. A
 * page asks for them again, filled in as last set, once the player has gone over an hour without
 * mouse or keyboard input on the site.
 */

const PROFILE_KEY = "cake-cut.profile";
const SEAT_KEY = "cake-cut.seat";
const LAST_INPUT_KEY = "cake-cut.last-input-ms";
/** Inactivity past which a page asks for the name and preference again. */
const INACTIVE_MS = 60 * 60 * 1000;
/** How often input refreshes the stored time, far finer than the hour it is checked against, so a mouse move does not write storage every frame. */
const SAVE_INTERVAL_MS = 10_000;
/** How often an open page checks whether the player has gone inactive. */
const CHECK_INTERVAL_MS = 30_000;
const INPUT_EVENTS = ["pointerdown", "pointermove", "keydown", "wheel"] as const;
/** Where a new player's slider starts: no preference either way. */
export const DEFAULT_STANDUP_PREFERENCE = 0.5;

/** The profile last joined with on this browser, or none. */
export function loadProfile(): Profile | null {
  const stored = readStorage(PROFILE_KEY);
  if (stored === null) return null;
  try {
    const parsed: unknown = JSON.parse(stored);
    return toProfile(parsed);
  } catch (error) {
    console.warn(`stored profile unreadable: ${error}`);
    return null;
  }
}

export function saveProfile(profile: Profile): void {
  writeStorage(PROFILE_KEY, JSON.stringify(profile));
}

/** The token the server last welcomed this browser's player with, which reopens their seat, or none. */
export function loadSeat(): string | null {
  return readStorage(SEAT_KEY);
}

export function saveSeat(seat: string): void {
  writeStorage(SEAT_KEY, seat);
}

/** Whether the player gave mouse or keyboard input on the site within the last hour. */
export function isActive(): boolean {
  const lastInputMs = Number(readStorage(LAST_INPUT_KEY));
  return Date.now() - lastInputMs <= INACTIVE_MS;
}

/** Records the player's input on this page from now on, the name dialog's included, for `isActive` in every tab. */
export function recordInput(): void {
  let savedMs = 0;
  listenInput(() => {
    const nowMs = Date.now();
    if (nowMs - savedMs < SAVE_INTERVAL_MS) return;
    savedMs = nowMs;
    writeStorage(LAST_INPUT_KEY, String(nowMs));
  });
}

/**
 * Calls `onInactive` once the player has gone over an hour without input on the site, then again
 * only after input on this page and another such hour. A page opened after such an hour, with no
 * input since, counts as already told.
 */
export function watchActivity(onInactive: () => void): void {
  let isTold = !isActive();
  listenInput(() => {
    isTold = false;
  });
  setInterval(() => {
    if (isTold || isActive()) return;
    isTold = true;
    onInactive();
  }, CHECK_INTERVAL_MS);
}

function listenInput(onInput: () => void): void {
  // Captured on the window, so a dialog that stops its keys from reaching the camera still counts them.
  for (const type of INPUT_EVENTS) window.addEventListener(type, onInput, { capture: true, passive: true });
}

/** The stored `value` as a profile, or none when it is not one. A profile stored before the scrum update checkbox existed has an update, as the checkbox starts checked. */
function toProfile(value: unknown): Profile | null {
  if (typeof value !== "object" || value === null) return null;
  const { name, standupPreference, hasScrumUpdate = true } = value as Record<string, unknown>;
  if (typeof name !== "string" || typeof hasScrumUpdate !== "boolean") return null;
  if (typeof standupPreference !== "number" || standupPreference < 0 || standupPreference > 1) return null;
  return { name, standupPreference, hasScrumUpdate };
}

/** The stored value, or none when it is missing or the browser refuses storage, as some private modes do. */
function readStorage(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch (error) {
    console.warn(`reading ${key} from storage: ${error}`);
    return null;
  }
}

function writeStorage(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch (error) {
    console.warn(`writing ${key} to storage: ${error}`);
  }
}
