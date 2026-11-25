import { Game } from "@wasm/cake_wasm.js";
import { type CakeEntry, fetchCakeFile } from "./cakes";
import { type LoadedCake, type Scene, disposeLoadedCake } from "./scene";

/** A round's cake ready to swap in: its game at tick 0, cutting itself at `ticks` or never, and its drawable cake, already on the GPU. */
export interface RoundCake {
  cake: CakeEntry;
  ticks: number | undefined;
  game: Game;
  loaded: LoadedCake;
}

/** Downloads `cake` and builds its round, spread over frames as `Scene.loadCake` describes. */
async function buildRoundCake(scene: Scene, cake: CakeEntry, ticks: number | undefined): Promise<RoundCake> {
  const game = new Game(ticks, await fetchCakeFile(cake));
  try {
    return { cake, ticks, game, loaded: await scene.loadCake(game) };
  } catch (error) {
    game.free();
    throw error;
  }
}

export function freeRoundCake(roundCake: RoundCake): void {
  roundCake.game.free();
  disposeLoadedCake(roundCake.loaded);
}

/**
 * Builds the lobby's picked cake ahead, hidden, so the round on it starts without a load. It holds
 * at most one, the latest pick, and frees any it drops.
 */
export class CakePreloader {
  private pending: { cake: CakeEntry; ticks: number | undefined; built: Promise<RoundCake | null> } | null = null;

  constructor(private readonly scene: Scene) {}

  /** Starts building `cake`'s round, dropping the one built before, unless that is already it. */
  preload(cake: CakeEntry, ticks: number | undefined): void {
    if (this.pending?.cake.id === cake.id && this.pending.ticks === ticks) return;
    this.drop();
    const built = buildRoundCake(this.scene, cake, ticks).catch((error: unknown) => {
      console.error(`preloading cake ${cake.id} failed:`, error);
      return null;
    });
    this.pending = { cake, ticks, built };
  }

  /**
   * `cake`'s round, owned by the caller: the one built ahead, waiting for it if it is still
   * building, or one built now if none was or its build failed.
   */
  async take(cake: CakeEntry, ticks: number | undefined): Promise<RoundCake> {
    const pending = this.pending;
    if (pending?.cake.id !== cake.id || pending.ticks !== ticks) return buildRoundCake(this.scene, cake, ticks);
    this.pending = null;
    return (await pending.built) ?? buildRoundCake(this.scene, cake, ticks);
  }

  /** Frees the cake built ahead, once its build ends. */
  drop(): void {
    const pending = this.pending;
    this.pending = null;
    void pending?.built.then((built) => built && freeRoundCake(built));
  }
}
