//! The one game room a server holds: its players, host, pick, and round.
//!
//! Events from connections and [`Room::step`], when [`Room::get_next_step`] says, advance it. It reads the time only
//! from its callers, so tests can drive it on a made-up clock.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{BuildHasher, RandomState};
use std::sync::Arc;

use cake_core::cut::MAX_SCORE;
use cake_core::input::Input;
use cake_core::sim::{Cake, ROUND_TICKS, TICKS_PER_S};
use rayon::prelude::*;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::oneshot;

use crate::bot::{BotPlay, pick_standup_preference};
use crate::catalog::Catalog;
use crate::protocol::{
    Member, POSE_RECORD_SIZE, PhaseName, PlayerId, RANDOM_CAKE, Run, ServerMessage, Standing, build_pose,
    encode_pose_record,
};
use crate::replay::{Note, Replay};
use crate::standup::{Chooser, calc_standup_order, order_choosers, shuffle_randomly};

/// Makes any player in the lobby or the results host, even with a host present.
pub const HOST_PASSWORD: &str = "greylag";
/// Steps between pose frames, which stream every round player's world to every client.
const POSE_INTERVAL_STEPS: u32 = 2;
/// The most ticks one step holds a world on toward the round's clock beyond the clock's own
/// advance since the last step. A world that joined the round late catches up over steps, each
/// costing about 3 ms on a detailed cake, rather than in one step of a whole round's ticks, about
/// 170 ms. It still catches up within the grace of the round's last tick, so the deadline finds
/// it cut.
const MAX_HELD_TICKS_PER_STEP: u32 = 30;
/// Bot ids count up from just past this, far above any connection's, so the two never meet.
const BOT_ID_BASE: PlayerId = 1 << 30;
/// The most members, pages and bots together, a room may hold for a bot to be added, as many as
/// a round holds. A page joining is never refused by it.
const MAX_MEMBERS_FOR_BOT: usize = 16;
/// The round lengths, in seconds, a `?debug` page may set.
const ROUND_S_RANGE: std::ops::RangeInclusive<u32> = 1..=600;
/// The most latency each way a `?debug` page may give the bots, in milliseconds.
const MAX_BOT_LATENCY_MS: u32 = 10_000;
/// Frames a connection may have waiting to send, about 8 s of pose frames, before the room
/// disconnects it as too slow to follow the round.
pub const OUTBOX_LEN: usize = 256;

/// One connection's identity, never reused. A connection plays as the player whose seat it
/// joined into, which a later connection can take over once this one has gone.
pub type ConnectionId = u32;

#[derive(Clone, Debug)]
pub struct RoomConfig {
    /// The length of the rounds the room starts, which a `?debug` page may change.
    pub round_ticks: u32,
    /// How long a round waits for the players in the room at its start to load its cake before
    /// starting without them, who then play once loaded, with the time left.
    pub load_timeout_ms: u64,
    /// How far a connected player's world may trail their page's clock waiting for their inputs,
    /// beyond a resend's round trip, past which it steps on with their last input, so the
    /// deadline, the round's length plus this and twice the page's input lag, finds every world
    /// cut. A world whose page has gone keeps up with the clock instead.
    pub deadline_grace_ms: u64,
    /// The most a page's input lag, how far behind the round's clock its inputs arrive when nothing
    /// holds them up, counts. A page keeps to the round's clock from when `start` reached it, so
    /// its inputs arrive a round trip late, which the grace alone would take for a stall, and a
    /// lost packet holds its stream for about a round trip more while TCP resends it, so its world
    /// is held back by twice its input lag beyond the grace.
    pub max_input_lag_ms: u64,
    /// How many ticks past the round's clock an input or cut may be stamped. One stamped further
    /// is refused, so no client can make one step replay its world far ahead of real time.
    pub max_lead_ticks: u32,
    /// How long a player whose page closed keeps their seat past their last activity, for the page
    /// to reopen into.
    pub seat_hold_ms: u64,
    /// How long the judging waits after the round's last cut, for every page's opponent views to
    /// play it: the 2.2 s knife motion plus their playback lag.
    pub judging_delay_ms: u64,
    /// How long each judging scene shows a player's halves together, their counters at 0, before
    /// the counting starts.
    pub judging_hold_ms: u64,
    /// How long a perfect cut's gram count takes in its judging scene, the longest any takes: a
    /// worse cut's is shorter by the square of its score's share of a perfect one.
    pub judging_max_count_ms: u64,
    /// How long each judging scene goes on after its count ends and the score shows, before the next.
    pub judging_after_ms: u64,
    /// How long the judging reveal holds after its last scene, with the leaderboard full, before
    /// the results cover it.
    pub judging_tail_ms: u64,
}

impl Default for RoomConfig {
    fn default() -> Self {
        Self {
            round_ticks: ROUND_TICKS,
            load_timeout_ms: 10_000,
            deadline_grace_ms: 2_000,
            max_input_lag_ms: 3_000,
            max_lead_ticks: TICKS_PER_S / 2,
            seat_hold_ms: 60 * 60 * 1000,
            judging_delay_ms: 3_000,
            judging_hold_ms: 500,
            judging_max_count_ms: 3_000,
            judging_after_ms: 500,
            judging_tail_ms: 1_500,
        }
    }
}

/// When the room next needs a [`Room::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextStep {
    /// Every tick of the simulation, while a round loads, plays, or is judged.
    EveryTick,
    /// At `at_ms`, when the earliest held seat expires.
    At { at_ms: u64 },
    /// None until an event, since nothing in the room changes with time alone.
    AfterEvent,
}

/// A frame for one connection to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
}

/// Where the room puts a connection's frames, holding at most [`OUTBOX_LEN`].
pub type Outbox = Sender<Frame>;

/// A connection's outbox and the end its connection sends from.
#[must_use]
pub fn create_outbox() -> (Outbox, Receiver<Frame>) {
    channel(OUTBOX_LEN)
}

/// What a connection tells the room, already validated. Every event names the connection it came
/// from, so one from a connection the room has since replaced or disconnected is ignored.
pub enum RoomEvent {
    /// A new connection, which plays as a fresh player of the connection's own id unless its name
    /// or its `seat` token reopens a seat, and is told on `assigned` which id it plays as, or that
    /// its name is taken, which seats nothing.
    ///
    /// No two players in the room, connected or holding a seat, share a name, compared ignoring
    /// case and surrounding spaces. A name matching a held seat's takes that seat over, token
    /// included. A name matching a connected player's is refused, unless the `seat` token is that
    /// player's own, which reopens their seat here and closes their other connection, as a
    /// reloaded page does whose old connection the server has yet to see close.
    ///
    /// A joiner while a round loads, or plays until every round player has cut, joins it, and
    /// plays it once their page has loaded its cake, with the time left.
    Join {
        connection: ConnectionId,
        name: String,
        /// From 0, first to give their standup update, to 1, last, or none without an update.
        standup_preference: Option<f64>,
        seat: Option<String>,
        /// Whether the page was opened with `?debug`, which alone may send [`RoomEvent::Debug`].
        is_debug: bool,
        outbox: Outbox,
        assigned: oneshot::Sender<Result<PlayerId, NameTaken>>,
    },
    Leave {
        connection: ConnectionId,
    },
    ClaimHost {
        connection: ConnectionId,
        password: String,
    },
    Pick {
        connection: ConnectionId,
        cake: String,
    },
    Start {
        connection: ConnectionId,
    },
    ReturnToLobby {
        connection: ConnectionId,
    },
    Ready {
        connection: ConnectionId,
    },
    Input {
        connection: ConnectionId,
        tick: u32,
        input: Input,
    },
    Cut {
        connection: ConnectionId,
        tick: u32,
    },
    Hash {
        connection: ConnectionId,
        tick: u32,
        hash: u64,
    },
    /// A change to the room's debug settings, refused from a page not opened with `?debug` and from any but the host.
    Debug {
        connection: ConnectionId,
        command: DebugCommand,
    },
}

/// What a `?debug` page may change in the room, at any time. Every change is logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugCommand {
    /// Adds a bot, which joins a round in progress as a late joiner does.
    AddBot,
    /// Removes the newest bot, which a round in progress keeps as a leaver.
    RemoveBot,
    /// Sets the length of the rounds started from now on, in seconds.
    SetRoundLength { round_s: u32 },
    /// Sets how long each message between a bot and the room takes, each way.
    SetBotLatency { latency_ms: u32 },
}

impl RoomEvent {
    const fn get_connection(&self) -> ConnectionId {
        match *self {
            Self::Join { connection, .. }
            | Self::Leave { connection }
            | Self::ClaimHost { connection, .. }
            | Self::Pick { connection, .. }
            | Self::Start { connection }
            | Self::ReturnToLobby { connection }
            | Self::Ready { connection }
            | Self::Input { connection, .. }
            | Self::Cut { connection, .. }
            | Self::Hash { connection, .. }
            | Self::Debug { connection, .. } => connection,
        }
    }
}

struct Player {
    /// The connection playing this seat, the only one whose events the room takes as the player's.
    connection: ConnectionId,
    seat: Seat,
    /// The secret that reopens this seat after the page closes.
    token: String,
    /// When the player last did something themselves, as opposed to their page reporting in.
    last_active_ms: u64,
    /// Whether the page was opened with `?debug`, so it may change the room's debug settings and hears them.
    is_debug: bool,
    outbox: Outbox,
}

/// A connection joining the room, as the seat it asks for.
struct Joining {
    connection: ConnectionId,
    seat: Seat,
    is_debug: bool,
    outbox: Outbox,
}

/// A simulated player a `?debug` page added, in the room until one removes it.
struct Bot {
    seat: Seat,
    /// The bot's own count among the bots added, which varies its play.
    number: u32,
    /// Its play of the round in progress, if it is in one.
    play: Option<BotPlay>,
}

/// A join refused for a name a player whose page is connected already has.
#[derive(Debug, PartialEq, Eq)]
pub struct NameTaken;

#[derive(Clone)]
struct Seat {
    name: String,
    /// From 0, first to give their standup update, to 1, last, or none for a player without a
    /// scrum update, who gets no standup slot. No one else sees it before the results.
    standup_preference: Option<f64>,
}

/// A seat whose page closed, held for it to reopen into, its name still the player's.
struct HeldSeat {
    seat: Seat,
    token: String,
    last_active_ms: u64,
}

/// Who already has a joiner's name.
enum Namesake {
    Connected,
    Held(PlayerId),
}

/// The next round's cake, always a catalog id, so every page can download it in the lobby, and
/// whether the host picked Random, which settled on it at once and is never rerolled at the start.
struct Pick {
    cake_id: String,
    is_random: bool,
}

/// A round's players stay in it to the end, judged at the deadline if their page closed or never
/// loaded the cake.
struct Round {
    cake_id: String,
    cake: Arc<Cake>,
    length_ticks: u32,
    replays: BTreeMap<PlayerId, Replay>,
    /// Each round player as they started or joined, so a player whose page closed is still named
    /// and seated.
    seats: BTreeMap<PlayerId, Seat>,
    /// Round players whose page has loaded the cake, or left, so every page shows their view.
    loaded: BTreeSet<PlayerId>,
    /// Round players whose cut everyone has been told of.
    announced_cuts: BTreeSet<PlayerId>,
}

impl Round {
    /// Puts player `id`, seated as `seat`, in the round if they are not in it yet, with a fresh
    /// world that keeps up with the round's clock while their page loads the cake.
    fn ensure_player(&mut self, id: PlayerId, seat: &Seat) {
        if self.replays.contains_key(&id) {
            return;
        }
        self.replays.insert(id, Replay::create(self.cake.clone(), self.length_ticks));
        self.seats.insert(id, seat.clone());
        eprintln!("{} (#{id}) joins the round in progress", seat.name);
    }
}

#[derive(Default)]
enum Phase {
    #[default]
    Lobby,
    Loading {
        round: Round,
        /// The players the round's message named, those in the room at the start, whom the
        /// start waits for to load the cake, up to the load timeout. A joiner since then does not
        /// hold it up, and every page adds their view only from `now_playing`.
        named: BTreeSet<PlayerId>,
        since_ms: u64,
    },
    Playing(Play),
    /// The reveal every page shows in step, judging each player in `order` for a scene apiece,
    /// whose gram count takes the matching `count_ms`.
    Judging {
        round: Round,
        standings: Vec<Standing>,
        order: Vec<PlayerId>,
        count_ms: Vec<u64>,
        start_ms: u64,
    },
    /// Until the host returns everyone to the lobby.
    Results {
        round: Round,
        standings: Vec<Standing>,
        standup: Vec<Member>,
    },
}

struct Play {
    round: Round,
    start_ms: u64,
    /// The round's clock at the last step.
    last_clock_tick: u32,
    steps_since_poses: u32,
    /// Once every player has cut, when the judging starts.
    judging_at_ms: Option<u64>,
}

impl Phase {
    const fn find_round(&self) -> Option<&Round> {
        match self {
            Self::Lobby => None,
            Self::Loading { round, .. }
            | Self::Playing(Play { round, .. })
            | Self::Judging { round, .. }
            | Self::Results { round, .. } => Some(round),
        }
    }

    /// The round a joiner plays: one loading, or in play until every round player has cut.
    const fn find_joinable_round_mut(&mut self) -> Option<&mut Round> {
        match self {
            Self::Loading { round, .. } | Self::Playing(Play { round, judging_at_ms: None, .. }) => Some(round),
            Self::Lobby | Self::Playing(_) | Self::Judging { .. } | Self::Results { .. } => None,
        }
    }
}

pub struct Room {
    config: RoomConfig,
    catalog: Catalog,
    /// The players whose page is connected.
    players: BTreeMap<PlayerId, Player>,
    /// The seats whose page closed, until their player has been inactive for `seat_hold_ms`.
    held_seats: BTreeMap<PlayerId, HeldSeat>,
    /// Vacant until someone claims it with [`HOST_PASSWORD`], and again once the host leaves.
    host: Option<PlayerId>,
    pick: Pick,
    phase: Phase,
    /// The bots `?debug` pages added, which count as connected players but for seeing the room,
    /// so a room of bots alone is left to no one.
    bots: BTreeMap<PlayerId, Bot>,
    bots_added: u32,
    /// How long each message between a bot and the room takes, each way.
    bot_latency_ms: u64,
}

impl Room {
    #[must_use]
    pub fn create(config: RoomConfig, catalog: Catalog) -> Self {
        let pick = Pick { cake_id: catalog.pick_random_id().to_string(), is_random: true };
        Self {
            config,
            catalog,
            players: BTreeMap::new(),
            held_seats: BTreeMap::new(),
            host: None,
            pick,
            phase: Phase::Lobby,
            bots: BTreeMap::new(),
            bots_added: 0,
            bot_latency_ms: 0,
        }
    }

    pub fn on_event(&mut self, event: RoomEvent, now_ms: u64) {
        match event {
            RoomEvent::Join { connection, name, standup_preference, seat, is_debug, outbox, assigned } => {
                let joining = Joining { connection, seat: Seat { name, standup_preference }, is_debug, outbox };
                let joined = self.join(joining, seat.as_deref(), now_ms);
                // The connection may have closed meanwhile; its leave event follows.
                let _ = assigned.send(joined);
            }
            event => {
                // A connection the room replaced or disconnected may still have events on their way.
                if let Some(id) = self.find_player_id(event.get_connection()) {
                    self.on_player_event(id, event, now_ms);
                }
            }
        }
        self.disconnect_overflowed();
    }

    /// Takes `event` from the connection playing as player `id`.
    fn on_player_event(&mut self, id: PlayerId, event: RoomEvent, now_ms: u64) {
        match event {
            RoomEvent::Join { .. } => unreachable!("a join comes from a connection no player has yet"),
            RoomEvent::Leave { .. } => self.leave(id),
            RoomEvent::ClaimHost { password, .. } => {
                self.touch(id, now_ms);
                self.claim_host(id, &password);
            }
            RoomEvent::Pick { cake, .. } => {
                self.touch(id, now_ms);
                self.pick_cake(id, &cake);
            }
            RoomEvent::Start { .. } => {
                self.touch(id, now_ms);
                self.start_loading(id, now_ms);
            }
            RoomEvent::ReturnToLobby { .. } => {
                self.touch(id, now_ms);
                self.return_to_lobby(id);
            }
            RoomEvent::Ready { .. } => self.mark_loaded(id, now_ms),
            RoomEvent::Input { tick, input, .. } => {
                // A page sends an input every tick of play, so only one that changed is the player's own doing.
                if let Some((replay, clock_tick)) = self.find_replay_within_lead(id, "input", tick, now_ms) {
                    let is_changed = replay.find_last_pushed_input() != Some(input);
                    replay.note_input_arrival(tick, clock_tick);
                    replay.push_input(tick, input);
                    if is_changed {
                        self.touch(id, now_ms);
                    }
                }
            }
            RoomEvent::Cut { tick, .. } => {
                if let Some((replay, _)) = self.find_replay_within_lead(id, "cut", tick, now_ms) {
                    replay.push_cut(tick);
                }
                self.touch(id, now_ms);
            }
            RoomEvent::Hash { tick, hash, .. } => {
                if let Some(replay) = self.find_replay(id) {
                    replay.push_hash(tick, hash);
                }
            }
            RoomEvent::Debug { command, .. } => {
                self.touch(id, now_ms);
                self.debug(id, command, now_ms);
            }
        }
    }

    /// Advances the round one tick of wall time: replays what clients and bots sent, holds each
    /// world to the round's clock, streams poses, and moves between phases. Like every event, it
    /// ends by disconnecting any player whose outbox filled.
    pub fn step(&mut self, now_ms: u64) {
        self.step_bots(now_ms);
        self.step_phase(now_ms);
        self.expire_held_seats(now_ms);
        self.disconnect_overflowed();
    }

    /// When the room next needs a [`Room::step`], which an event or a step may change, so a caller
    /// asks again after each. A step sooner than this changes nothing.
    #[must_use]
    pub fn get_next_step(&self) -> NextStep {
        if matches!(self.phase, Phase::Loading { .. } | Phase::Playing(_) | Phase::Judging { .. }) {
            return NextStep::EveryTick;
        }
        let hold_ms = self.config.seat_hold_ms;
        let earliest_expiry_ms = self.held_seats.values().map(|held| calc_seat_expiry_ms(held, hold_ms)).min();
        earliest_expiry_ms.map_or(NextStep::AfterEvent, |at_ms| NextStep::At { at_ms })
    }

    fn find_player_id(&self, connection: ConnectionId) -> Option<PlayerId> {
        self.players.iter().find(|(_, player)| player.connection == connection).map(|(&id, _)| id)
    }

    fn expire_held_seats(&mut self, now_ms: u64) {
        let hold_ms = self.config.seat_hold_ms;
        self.held_seats.retain(|id, held| {
            let is_kept = now_ms < calc_seat_expiry_ms(held, hold_ms);
            if !is_kept {
                eprintln!("#{id}: seat expired");
            }
            is_kept
        });
    }

    /// Notes that player `id` did something themselves just now.
    fn touch(&mut self, id: PlayerId, now_ms: u64) {
        if let Some(player) = self.players.get_mut(&id) {
            player.last_active_ms = now_ms;
        }
    }

    /// Steps a round in play, and moves the round on to its next phase when due. A phase taken
    /// out to step leaves Lobby in `self.phase` until it or the next is put back, so nothing in
    /// between may read it.
    fn step_phase(&mut self, now_ms: u64) {
        match std::mem::take(&mut self.phase) {
            Phase::Loading { round, named, since_ms }
                if named.is_subset(&round.loaded) || now_ms >= since_ms + self.config.load_timeout_ms =>
            {
                self.start_playing(round, since_ms, now_ms);
            }
            Phase::Playing(mut play) => {
                if self.step_play(&mut play, now_ms) {
                    self.start_judging(play.round, now_ms);
                } else {
                    self.phase = Phase::Playing(play);
                }
            }
            phase => self.phase = phase,
        }
        // After the above, so a reveal of no length gives way to the results in the step that starts it.
        match std::mem::take(&mut self.phase) {
            Phase::Judging { round, standings, count_ms, start_ms, .. }
                if now_ms >= start_ms + self.calc_judging_ms(&count_ms) =>
            {
                self.show_results(round, standings);
            }
            phase => self.phase = phase,
        }
    }

    /// Seats `joining` as [`RoomEvent::Join`] describes and returns the id it plays as. An
    /// expired seat is not reopened.
    fn join(&mut self, joining: Joining, token: Option<&str>, now_ms: u64) -> Result<PlayerId, NameTaken> {
        let Joining { connection, seat, is_debug, outbox } = joining;
        let own_id = token.and_then(|token| self.find_seat_id_by_token(token));
        let reopened_id = match self.find_namesake(&seat.name, own_id) {
            Some(Namesake::Connected) => {
                eprintln!("{}: join refused, a connected player has the same name", seat.name);
                return Err(NameTaken);
            }
            Some(Namesake::Held(id)) => Some(id),
            None => own_id,
        };
        if let Some(id) = reopened_id
            && self.players.contains_key(&id)
        {
            eprintln!("{} (#{id}) reopened on another connection, closing the old one", seat.name);
            self.hold_seat(id);
        }
        let (id, token) = match reopened_id {
            Some(id) => (id, self.held_seats.remove(&id).expect("a reopened seat is held").token),
            None => (connection, create_seat_token(connection)),
        };
        eprintln!("{} (#{id}) {}", seat.name, if reopened_id.is_some() { "rejoined" } else { "joined" });
        // The page left while the round loaded, which marked it loaded, but its reopened page must
        // load the cake again, as any page does.
        if let Phase::Loading { round, .. } = &mut self.phase {
            round.loaded.remove(&id);
        }
        let player = Player { connection, seat: seat.clone(), token, last_active_ms: now_ms, is_debug, outbox };
        send(&player, &ServerMessage::Welcome { id, seat: player.token.clone() });
        self.players.insert(id, player);
        if let Some(round) = self.phase.find_joinable_round_mut() {
            round.ensure_player(id, &seat);
        }
        self.broadcast_room();
        self.send_round_state(id, now_ms);
        if is_debug {
            send(&self.players[&id], &self.build_debug_message());
        }
        Ok(id)
    }

    /// The seat `token` opens, connected or held.
    fn find_seat_id_by_token(&self, token: &str) -> Option<PlayerId> {
        let connected = self.players.iter().map(|(&id, player)| (id, &player.token));
        let held = self.held_seats.iter().map(|(&id, held)| (id, &held.token));
        connected.chain(held).find(|&(_, seat_token)| seat_token == token).map(|(id, _)| id)
    }

    /// Who other than seat `own_id` has `name`, connected or holding a seat.
    fn find_namesake(&self, name: &str, own_id: Option<PlayerId>) -> Option<Namesake> {
        let folded = fold_name(name);
        let is_namesake = |id: PlayerId, seat: &Seat| Some(id) != own_id && fold_name(&seat.name) == folded;
        let is_bot_namesake = self.bots.iter().any(|(&id, bot)| is_namesake(id, &bot.seat));
        if is_bot_namesake || self.players.iter().any(|(&id, player)| is_namesake(id, &player.seat)) {
            return Some(Namesake::Connected);
        }
        self.held_seats.iter().find(|&(&id, held)| is_namesake(id, &held.seat)).map(|(&id, _)| Namesake::Held(id))
    }

    /// Tells player `id`, new to the room, what it is doing: the pick in the lobby, or else the
    /// round so far, and the steps of their own world if they are in it.
    fn send_round_state(&self, id: PlayerId, now_ms: u64) {
        let player = &self.players[&id];
        let (round, started_ms) = match &self.phase {
            Phase::Lobby => {
                send(player, &self.build_pick_message());
                return;
            }
            Phase::Playing(play) => (&play.round, Some(play.start_ms)),
            Phase::Loading { round, .. } | Phase::Judging { round, .. } | Phase::Results { round, .. } => (round, None),
        };
        send(player, &build_round_message(round));
        if let Some(start_ms) = started_ms {
            send(player, &ServerMessage::Start { elapsed_ms: now_ms - start_ms });
            let still_loading = find_still_loading(round);
            if !still_loading.is_empty() {
                send(player, &ServerMessage::StillLoading { players: still_loading });
            }
        }
        for &cut_id in &round.announced_cuts {
            send(player, &build_cut_message(cut_id, &round.replays[&cut_id]));
        }
        if let Some(replay) = round.replays.get(&id) {
            send(player, &build_resume_message(replay));
        }
        match &self.phase {
            Phase::Judging { standings, order, count_ms, start_ms, .. } => {
                send(player, &self.build_judging_message(standings, order, count_ms, now_ms - start_ms));
            }
            Phase::Results { standings, standup, .. } => {
                send(player, &ServerMessage::Results { standings: standings.clone(), standup: standup.clone() });
            }
            _ => {}
        }
    }

    /// Holds the seat of a player whose page closed and tells everyone left.
    fn leave(&mut self, id: PlayerId) {
        self.hold_seat(id);
        // A reveal or results no one is left to see would greet the next joiner, so the room moves on.
        if self.players.is_empty() && matches!(self.phase, Phase::Judging { .. } | Phase::Results { .. }) {
            self.enter_lobby();
            return;
        }
        self.broadcast_room();
    }

    /// Holds the seat of connected player `id`, whose page has gone. A round they are in plays on
    /// without them: their world lets go of the cake and knife, and keeps up with the round's
    /// clock to its cut.
    fn hold_seat(&mut self, id: PlayerId) {
        let player = self.players.remove(&id).expect("only a connected player leaves");
        eprintln!("{} (#{id}) left", player.seat.name);
        let held = HeldSeat { seat: player.seat, token: player.token, last_active_ms: player.last_active_ms };
        self.held_seats.insert(id, held);
        if self.host == Some(id) {
            self.host = None;
        }
        self.release_from_round(id);
    }

    /// Lets the round in progress play on without player `id`, who has gone: their world lets go
    /// of the cake and knife, and keeps up with the round's clock to its cut.
    fn release_from_round(&mut self, id: PlayerId) {
        match &mut self.phase {
            Phase::Lobby | Phase::Judging { .. } | Phase::Results { .. } => {}
            Phase::Loading { round, named, .. } => {
                // Nothing waits on a page that has gone, and its world needs no release before it
                // has stepped, which would take the place of a reopened page's first input. Every
                // page shows it as a leaver, as it does those the round's message named.
                if round.replays.contains_key(&id) && round.loaded.insert(id) && !named.contains(&id) {
                    broadcast(&self.players, &build_now_playing_message(round, id));
                }
            }
            Phase::Playing(play) => {
                if let Some(replay) = play.round.replays.get_mut(&id) {
                    replay.push_release();
                }
            }
        }
    }

    /// Takes player `id`'s word that their page has loaded the round's cake: the start no longer
    /// waits for them, and a round in progress has every page show their view.
    fn mark_loaded(&mut self, id: PlayerId, now_ms: u64) {
        let (round, named, clock_tick) = match &mut self.phase {
            Phase::Loading { round, named, .. } => (round, Some(named), 0),
            Phase::Playing(play) => (&mut play.round, None, calc_ticks(now_ms.saturating_sub(play.start_ms))),
            Phase::Lobby | Phase::Judging { .. } | Phase::Results { .. } => return,
        };
        let Some(replay) = round.replays.get_mut(&id) else {
            return;
        };
        replay.ensure_ready_noted(clock_tick);
        if !round.loaded.insert(id) {
            return;
        }
        // Every page already shows a player the round's message named.
        if named.is_some_and(|named| named.contains(&id)) {
            return;
        }
        eprintln!("{} (#{id}) loaded the cake and plays the round in progress", round.seats[&id].name);
        broadcast(&self.players, &build_now_playing_message(round, id));
    }

    /// Makes player `id` host if `password` is the host password, outside a round.
    fn claim_host(&mut self, id: PlayerId, password: &str) {
        let player = &self.players[&id];
        if matches!(self.phase, Phase::Loading { .. } | Phase::Playing(_) | Phase::Judging { .. }) {
            send(player, &ServerMessage::Error { message: "host can be claimed only outside a round".to_string() });
            return;
        }
        if password != HOST_PASSWORD {
            eprintln!("{} (#{id}) claimed host with the wrong password", player.seat.name);
            send(player, &ServerMessage::ClaimRefused);
            return;
        }
        eprintln!("{} (#{id}) claimed host", player.seat.name);
        self.host = Some(id);
        self.broadcast_room();
    }

    /// Settles the next round's cake on the host's pick, a random one for [`RANDOM_CAKE`].
    fn pick_cake(&mut self, id: PlayerId, cake: &str) {
        if let Err(message) = self.validate_pick(id, cake) {
            send(&self.players[&id], &ServerMessage::Error { message });
            return;
        }
        self.pick = match cake {
            RANDOM_CAKE => self.settle_random_pick(),
            _ => Pick { cake_id: cake.to_string(), is_random: false },
        };
        eprintln!("{} (#{id}) picked {cake}: {}", self.players[&id].seat.name, self.pick.cake_id);
        self.broadcast(&self.build_pick_message());
    }

    fn validate_pick(&self, id: PlayerId, cake: &str) -> Result<(), String> {
        self.validate_host_in_lobby(id)?;
        if cake != RANDOM_CAKE && self.catalog.find(cake).is_none() {
            return Err(format!("no cake {cake:?}"));
        }
        Ok(())
    }

    fn validate_host_in_lobby(&self, id: PlayerId) -> Result<(), String> {
        if self.host != Some(id) {
            return Err("only the host can pick the cake or start a round".to_string());
        }
        if !matches!(self.phase, Phase::Lobby) {
            return Err("a round is already on".to_string());
        }
        Ok(())
    }

    fn settle_random_pick(&self) -> Pick {
        Pick { cake_id: self.catalog.pick_random_id().to_string(), is_random: true }
    }

    fn start_loading(&mut self, id: PlayerId, now_ms: u64) {
        if let Err(message) = self.validate_host_in_lobby(id) {
            send(&self.players[&id], &ServerMessage::Error { message });
            return;
        }
        let cake_id = self.pick.cake_id.clone();
        let cake = self.catalog.find(&cake_id).expect("the pick is a catalog cake").clone();
        let round_ticks = self.config.round_ticks;
        let seats: BTreeMap<PlayerId, Seat> = self
            .players
            .iter()
            .map(|(&id, player)| (id, player.seat.clone()))
            .chain(self.bots.iter().map(|(&id, bot)| (id, bot.seat.clone())))
            .collect();
        let ids: Vec<PlayerId> = seats.keys().copied().collect();
        let replays = ids.par_iter().map(|&id| (id, Replay::create(cake.clone(), round_ticks))).collect();
        eprintln!("round on {cake_id} for {} players, {round_ticks} ticks", seats.len());
        for bot in self.bots.values_mut() {
            bot.play = Some(BotPlay::create(cake.get_model(), now_ms + 2 * self.bot_latency_ms));
        }
        let round = Round {
            cake_id,
            cake,
            length_ticks: round_ticks,
            replays,
            seats,
            loaded: BTreeSet::new(),
            announced_cuts: BTreeSet::new(),
        };
        let message = build_round_message(&round);
        self.phase = Phase::Loading { round, named: ids.into_iter().collect(), since_ms: now_ms };
        self.broadcast_room();
        self.broadcast(&message);
    }

    /// Ends the results for everyone, at the host's word.
    fn return_to_lobby(&mut self, id: PlayerId) {
        let player = &self.players[&id];
        if self.host != Some(id) || !matches!(self.phase, Phase::Results { .. }) {
            send(player, &ServerMessage::Error { message: "only the host can end the results".to_string() });
            return;
        }
        eprintln!("{} (#{id}) returned everyone to the lobby", player.seat.name);
        self.enter_lobby();
    }

    /// Moves everyone to the lobby with the pick preset to Random, settled at once.
    fn enter_lobby(&mut self) {
        self.phase = Phase::Lobby;
        self.pick = self.settle_random_pick();
        self.broadcast_room();
        self.broadcast(&self.build_pick_message());
    }

    /// Starts the round on time. A page that has not loaded the cake by now plays once it has,
    /// with the time left, and every page shows its view from then on.
    fn start_playing(&mut self, round: Round, since_ms: u64, now_ms: u64) {
        eprintln!(
            "round started {} ms after loading, {} of {} players ready",
            now_ms - since_ms,
            round.loaded.len(),
            round.replays.len()
        );
        let still_loading = find_still_loading(&round);
        for id in &still_loading {
            eprintln!("{} (#{id}) plays once the cake loads", round.seats[id].name);
        }
        if !still_loading.is_empty() {
            self.broadcast(&ServerMessage::StillLoading { players: still_loading });
        }
        let play = Play { round, start_ms: now_ms, last_clock_tick: 0, steps_since_poses: 0, judging_at_ms: None };
        self.phase = Phase::Playing(play);
        self.broadcast_room();
        self.broadcast(&ServerMessage::Start { elapsed_ms: 0 });
    }

    /// Replays what clients and bots sent, holds each world to the round's clock, and streams
    /// poses, returning whether the judging is due.
    fn step_play(&self, play: &mut Play, now_ms: u64) -> bool {
        let grace_ticks = calc_ticks(self.config.deadline_grace_ms);
        let max_input_lag_ticks = calc_ticks(self.config.max_input_lag_ms);
        let Play { round, start_ms, last_clock_tick, steps_since_poses, judging_at_ms } = play;
        let clock_tick = calc_ticks(now_ms.saturating_sub(*start_ms));
        // Beyond the clock's own advance, so a step on a clock that jumped ahead holds every world to it.
        let max_held_ticks = clock_tick.saturating_sub(*last_clock_tick) + MAX_HELD_TICKS_PER_STEP;
        *last_clock_tick = clock_tick;
        let players = &self.players;
        let bots = &self.bots;
        // Each world is independent and single-threaded, so the worlds step in parallel: one
        // world's tick with the knife lowered on a detailed cake can take most of the budget.
        let notes: Vec<(PlayerId, Vec<Note>)> = round
            .replays
            .par_iter_mut()
            .map(|(&id, replay)| {
                let notes = replay.drain();
                // A page that has gone sends nothing more, so its world keeps up with the clock and
                // cuts as time runs out, where a connected page's inputs get its round trip, a
                // resend's round trip, and the grace to arrive, so only a stall past them holds
                // its last input. A bot's inputs come from the room itself, with no round trip.
                let lag = match (players.contains_key(&id), bots.contains_key(&id)) {
                    (true, _) => {
                        grace_ticks + 2 * calc_page_lag_ticks(replay, clock_tick, max_input_lag_ticks, grace_ticks)
                    }
                    (false, true) => grace_ticks,
                    (false, false) => 0,
                };
                let caught_up_tick = replay.get_world().get_tick() + max_held_ticks;
                replay.hold_to(clock_tick.saturating_sub(lag).min(caught_up_tick));
                (id, notes)
            })
            .collect();
        for (id, notes) in notes {
            report_notes(&round.seats[&id], players.get(&id), id, &notes);
            let replay = round.replays.get_mut(&id).expect("notes come from a replay");
            if let Some(player) = players.get(&id)
                && replay.try_note_resync()
            {
                send(player, &build_resume_message(replay));
            }
        }
        announce_new_cuts(round, players);
        if judging_at_ms.is_none() && round.replays.values().all(Replay::is_cut) {
            *judging_at_ms = Some(now_ms + self.config.judging_delay_ms);
            broadcast(players, &ServerMessage::PlayOver);
        }
        if judging_at_ms.is_some_and(|at_ms| now_ms >= at_ms) {
            return true;
        }
        *steps_since_poses += 1;
        if *steps_since_poses >= POSE_INTERVAL_STEPS {
            *steps_since_poses = 0;
            broadcast_poses(round, players);
        }
        false
    }

    /// Ranks every cut and starts the reveal that judges each player in the order
    /// [`build_judging_order`] draws, which every page shows in step, then the results once its
    /// last scene and tail have played.
    fn start_judging(&mut self, round: Round, now_ms: u64) {
        let standings = rank(&round);
        for standing in &standings {
            eprintln!(
                "rank {}: {} (#{}) {} at tick {}",
                standing.rank, standing.name, standing.id, standing.score, standing.tick
            );
        }
        let judged = build_judging_order(&standings);
        let order: Vec<PlayerId> = judged.iter().map(|standing| standing.id).collect();
        let count_ms: Vec<u64> =
            judged.iter().map(|standing| calc_count_ms(standing, self.config.judging_max_count_ms)).collect();
        let scenes: Vec<String> =
            judged.iter().zip(&count_ms).map(|(standing, ms)| format!("{} ({ms} ms)", standing.name)).collect();
        eprintln!("judging order: {}, results in {} ms", scenes.join(", "), self.calc_judging_ms(&count_ms));
        // A round everyone's page left plays out to its judging, which no one is left to see.
        if self.players.is_empty() {
            self.enter_lobby();
            return;
        }
        let message = self.build_judging_message(&standings, &order, &count_ms, 0);
        self.phase = Phase::Judging { round, standings, order, count_ms, start_ms: now_ms };
        self.broadcast_room();
        self.broadcast(&message);
    }

    /// How long the judging reveal of scenes counting for `count_ms` lasts, tail included.
    fn calc_judging_ms(&self, count_ms: &[u64]) -> u64 {
        let config = &self.config;
        count_ms.iter().map(|ms| config.judging_hold_ms + ms + config.judging_after_ms).sum::<u64>()
            + config.judging_tail_ms
    }

    fn build_judging_message(
        &self,
        standings: &[Standing],
        order: &[PlayerId],
        count_ms: &[u64],
        elapsed_ms: u64,
    ) -> ServerMessage {
        ServerMessage::Judging {
            order: order.to_vec(),
            hold_ms: self.config.judging_hold_ms,
            count_ms: count_ms.to_vec(),
            after_ms: self.config.judging_after_ms,
            elapsed_ms,
            standings: standings.to_vec(),
        }
    }

    fn show_results(&mut self, round: Round, standings: Vec<Standing>) {
        let standup = self.build_standup(&round, &standings);
        let names: Vec<&str> = standup.iter().map(|member| member.name.as_str()).collect();
        eprintln!("standup order: {}", names.join(", "));
        let message = ServerMessage::Results { standings: standings.clone(), standup: standup.clone() };
        self.phase = Phase::Results { round, standings, standup };
        self.broadcast_room();
        self.broadcast(&message);
    }

    /// The Standup Order of everyone in the round or the room with a scrum update: the round's
    /// players by their `standings`, those whose page closed included, then the spectators in join
    /// order, which their ids follow.
    fn build_standup(&self, round: &Round, standings: &[Standing]) -> Vec<Member> {
        let ranked: Vec<(PlayerId, u32)> = standings.iter().map(|standing| (standing.id, standing.rank)).collect();
        let members: BTreeSet<PlayerId> = self.players.keys().chain(self.bots.keys()).copied().collect();
        let spectators: Vec<PlayerId> = members.into_iter().filter(|id| !round.replays.contains_key(id)).collect();
        let find_seat = |id: PlayerId| {
            round
                .seats
                .get(&id)
                .or_else(|| self.bots.get(&id).map(|bot| &bot.seat))
                .unwrap_or_else(|| &self.players[&id].seat)
        };
        let choosers: Vec<Chooser> = order_choosers(&ranked, &spectators, shuffle_randomly)
            .into_iter()
            .map(|id| Chooser { id, preference: find_seat(id).standup_preference })
            .collect();
        calc_standup_order(&choosers).into_iter().map(|id| Member { id, name: find_seat(id).name.clone() }).collect()
    }

    /// Changes the room's debug settings at player `id`'s word, if their page was opened with
    /// `?debug` and they are the host, and tells every `?debug` page.
    fn debug(&mut self, id: PlayerId, command: DebugCommand, now_ms: u64) {
        let player = &self.players[&id];
        if !player.is_debug || self.host != Some(id) {
            send(player, &ServerMessage::Error { message: "debug settings need the host's ?debug page".to_string() });
            return;
        }
        let name = player.seat.name.clone();
        match command {
            DebugCommand::AddBot => {
                if self.players.len() + self.bots.len() >= MAX_MEMBERS_FOR_BOT {
                    let message = format!("a bot joins only a room of under {MAX_MEMBERS_FOR_BOT} members");
                    send(&self.players[&id], &ServerMessage::Error { message });
                    return;
                }
                self.add_bot(&name, id, now_ms);
            }
            DebugCommand::RemoveBot => self.remove_bot(&name, id),
            DebugCommand::SetRoundLength { round_s } => {
                if !ROUND_S_RANGE.contains(&round_s) {
                    let message = format!("a round of {round_s} s is outside {ROUND_S_RANGE:?} s");
                    send(&self.players[&id], &ServerMessage::Error { message });
                    return;
                }
                self.config.round_ticks = round_s * TICKS_PER_S;
                eprintln!("{name} (#{id}) set the round length to {round_s} s");
                // Every page builds the pick ahead for the round's length, so it builds it again.
                if matches!(self.phase, Phase::Lobby) {
                    self.broadcast(&self.build_pick_message());
                }
            }
            DebugCommand::SetBotLatency { latency_ms } => {
                if latency_ms > MAX_BOT_LATENCY_MS {
                    let message = format!("a bot latency of {latency_ms} ms is over {MAX_BOT_LATENCY_MS} ms");
                    send(&self.players[&id], &ServerMessage::Error { message });
                    return;
                }
                self.bot_latency_ms = u64::from(latency_ms);
                eprintln!("{name} (#{id}) set the bots' latency to {latency_ms} ms each way");
            }
        }
        self.broadcast_debug();
    }

    /// Adds a bot at the word of `by` (#`by_id`), which joins a round loading or in play.
    fn add_bot(&mut self, by: &str, by_id: PlayerId, now_ms: u64) {
        self.bots_added += 1;
        let id = BOT_ID_BASE + self.bots_added;
        let seat = Seat { name: self.find_free_bot_name(), standup_preference: Some(pick_standup_preference()) };
        eprintln!("{by} (#{by_id}) added {} (#{id})", seat.name);
        let ready_at_ms = now_ms + 2 * self.bot_latency_ms;
        let play = self.phase.find_joinable_round_mut().map(|round| {
            round.ensure_player(id, &seat);
            BotPlay::create(round.cake.get_model(), ready_at_ms)
        });
        self.bots.insert(id, Bot { seat, number: self.bots_added, play });
        self.broadcast_room();
    }

    /// The lowest-numbered bot name no one in the room or its round has.
    fn find_free_bot_name(&self) -> String {
        let round_seats = self.phase.find_round().into_iter().flat_map(|round| round.seats.values());
        let seats = (self.players.values().map(|player| &player.seat))
            .chain(self.held_seats.values().map(|held| &held.seat))
            .chain(self.bots.values().map(|bot| &bot.seat))
            .chain(round_seats);
        let taken: BTreeSet<String> = seats.map(|seat| fold_name(&seat.name)).collect();
        // One more number than there are names, so at least one is free.
        (1..=taken.len() + 1)
            .map(|number| format!("Bot {number}"))
            .find(|name| !taken.contains(&fold_name(name)))
            .expect("some bot number is free")
    }

    /// Removes the newest bot at the word of `by` (#`by_id`), which a round in progress keeps as a
    /// leaver. With no bot, nothing changes.
    fn remove_bot(&mut self, by: &str, by_id: PlayerId) {
        let Some((id, bot)) = self.bots.pop_last() else {
            return;
        };
        eprintln!("{by} (#{by_id}) removed {} (#{id})", bot.seat.name);
        self.release_from_round(id);
        self.broadcast_room();
    }

    /// Sends each bot's `ready` and inputs that have reached the room by `now_ms`, its latency
    /// each way after the room's messages to it.
    fn step_bots(&mut self, now_ms: u64) {
        let ready: Vec<PlayerId> = self
            .bots
            .iter_mut()
            .filter_map(|(&id, bot)| bot.play.as_mut().is_some_and(|play| play.try_take_ready(now_ms)).then_some(id))
            .collect();
        for id in ready {
            self.mark_loaded(id, now_ms);
        }
        let Phase::Playing(Play { round, start_ms, .. }) = &mut self.phase else {
            return;
        };
        let arrived_tick = calc_ticks(now_ms.saturating_sub(*start_ms + 2 * self.bot_latency_ms));
        for (id, bot) in &mut self.bots {
            if let (Some(play), Some(replay)) = (bot.play.as_mut(), round.replays.get_mut(id)) {
                play.push_arrived(replay, bot.number, arrived_tick, round.length_ticks, MAX_HELD_TICKS_PER_STEP);
            }
        }
    }

    /// Tells every `?debug` page the room's debug settings.
    fn broadcast_debug(&self) {
        let message = self.build_debug_message();
        for player in self.players.values().filter(|player| player.is_debug) {
            send(player, &message);
        }
    }

    #[expect(clippy::cast_possible_truncation, reason = "a room's bots fit u32, and the latency is capped")]
    const fn build_debug_message(&self) -> ServerMessage {
        ServerMessage::Debug {
            round_s: self.config.round_ticks / TICKS_PER_S,
            bots: self.bots.len() as u32,
            bot_latency_ms: self.bot_latency_ms as u32,
        }
    }

    /// Player `id`'s replay, while the round is in play.
    fn find_replay(&mut self, id: PlayerId) -> Option<&mut Replay> {
        match &mut self.phase {
            Phase::Playing(play) => play.round.replays.get_mut(&id),
            _ => None,
        }
    }

    /// [`Room::find_replay`] with the round's clock tick at `now_ms`, unless the `kind` of message
    /// they stamped for `tick` is further ahead of that clock than the lead allows, which is
    /// refused and noted for the next step's log, one line however many arrive.
    fn find_replay_within_lead(
        &mut self,
        id: PlayerId,
        kind: &'static str,
        tick: u32,
        now_ms: u64,
    ) -> Option<(&mut Replay, u32)> {
        let Phase::Playing(Play { round, start_ms, .. }) = &mut self.phase else {
            return None;
        };
        let replay = round.replays.get_mut(&id)?;
        let clock_tick = calc_ticks(now_ms.saturating_sub(*start_ms));
        if u64::from(tick) > u64::from(clock_tick) + u64::from(self.config.max_lead_ticks) {
            replay.note_refused(kind, tick, clock_tick);
            return None;
        }
        Some((replay, clock_tick))
    }

    /// Disconnects every player whose outbox has filled, since their connection is not keeping up.
    fn disconnect_overflowed(&mut self) {
        // Each leave tells everyone left, which may fill another outbox.
        while let Some(id) = self.players.iter().find(|(_, player)| player.outbox.capacity() == 0).map(|(&id, _)| id) {
            eprintln!("#{id}: {OUTBOX_LEN} frames unsent, disconnecting");
            self.leave(id);
        }
    }

    fn build_pick_message(&self) -> ServerMessage {
        ServerMessage::Pick {
            cake: self.pick.cake_id.clone(),
            is_random: self.pick.is_random,
            round_ticks: self.config.round_ticks,
        }
    }

    fn broadcast_room(&self) {
        let phase = match self.phase {
            Phase::Lobby => PhaseName::Lobby,
            Phase::Loading { .. } => PhaseName::Loading,
            Phase::Playing(_) => PhaseName::Playing,
            Phase::Judging { .. } => PhaseName::Judging,
            Phase::Results { .. } => PhaseName::Results,
        };
        let seats = self
            .players
            .iter()
            .map(|(&id, player)| (id, &player.seat))
            .chain(self.bots.iter().map(|(&id, bot)| (id, &bot.seat)));
        let members = seats.map(|(id, seat)| Member { id, name: seat.name.clone() }).collect();
        self.broadcast(&ServerMessage::Room { phase, host: self.host, members });
    }

    fn broadcast(&self, message: &ServerMessage) {
        broadcast(&self.players, message);
    }
}

/// The first time `held` no longer reopens, once its player has been inactive over `hold_ms`.
const fn calc_seat_expiry_ms(held: &HeldSeat, hold_ms: u64) -> u64 {
    held.last_active_ms.saturating_add(hold_ms).saturating_add(1)
}

/// How many ticks behind the round's clock a page's inputs are taken to arrive at `clock_tick`:
/// its input lag, at most `max_ticks`. Before its first input has arrived, `max_ticks` until a
/// page that far would have been held up past its grace since it loaded the cake, so its first
/// inputs, which a player may make from the moment the round shows, are never stale; after that,
/// none, so a page that went silent is waited for no longer than a near one.
fn calc_page_lag_ticks(replay: &Replay, clock_tick: u32, max_ticks: u32, grace_ticks: u32) -> u32 {
    if let Some(ticks) = replay.find_input_lag_ticks() {
        return ticks.min(max_ticks);
    }
    let is_first_input_due =
        replay.find_ready_clock_tick().is_some_and(|ready_tick| clock_tick < ready_tick + grace_ticks + 2 * max_ticks);
    if is_first_input_due { max_ticks } else { 0 }
}

/// The whole ticks in `ms` of the simulation's time.
#[expect(clippy::cast_possible_truncation, reason = "u32 ticks last over two years")]
fn calc_ticks(ms: u64) -> u32 {
    (ms * u64::from(TICKS_PER_S) / 1000) as u32
}

/// Tells everyone of each cut made since the last step, logging its score.
fn announce_new_cuts(round: &mut Round, players: &BTreeMap<PlayerId, Player>) {
    for (&id, replay) in &round.replays {
        let Some((tick, split)) = replay.find_cut() else {
            continue;
        };
        if !round.announced_cuts.insert(id) {
            continue;
        }
        eprintln!("{} (#{id}) cut at tick {tick}: score {}", round.seats[&id].name, split.calc_score());
        broadcast(players, &build_cut_message(id, replay));
    }
}

fn build_round_message(round: &Round) -> ServerMessage {
    ServerMessage::Round {
        cake: round.cake_id.clone(),
        round_ticks: round.length_ticks,
        players: round.seats.iter().map(|(&id, seat)| Member { id, name: seat.name.clone() }).collect(),
    }
}

/// Round player `id`, who was not in the round's message or has loaded since its start, now plays.
fn build_now_playing_message(round: &Round, id: PlayerId) -> ServerMessage {
    ServerMessage::NowPlaying { player: Member { id, name: round.seats[&id].name.clone() } }
}

/// The round's players whose page has yet to load its cake.
fn find_still_loading(round: &Round) -> Vec<PlayerId> {
    round.replays.keys().filter(|id| !round.loaded.contains(id)).copied().collect()
}

/// Sends everyone one pose frame holding every round player's world.
fn broadcast_poses(round: &Round, players: &BTreeMap<PlayerId, Player>) {
    let mut frame = Vec::with_capacity(round.replays.len() * POSE_RECORD_SIZE);
    for (&id, replay) in &round.replays {
        encode_pose_record(&mut frame, id, replay.get_world());
    }
    for player in players.values() {
        send_frame(player, Frame::Binary(frame.clone()));
    }
}

fn build_resume_message(replay: &Replay) -> ServerMessage {
    let runs = replay.build_input_runs().into_iter().map(|run| Run { ticks: run.ticks, input: run.input.encode() });
    ServerMessage::Resume { runs: runs.collect(), cut_tick: replay.find_cut().map(|(tick, _)| tick) }
}

/// `name` as two names compare when the room tells them apart: ignoring case and surrounding spaces.
fn fold_name(name: &str) -> String {
    name.trim().to_lowercase()
}

/// A secret no other page can guess, which reopens seat `id` after its page closes.
fn create_seat_token(id: PlayerId) -> String {
    let random = RandomState::new();
    format!("{:016x}{:016x}", random.hash_one(id), random.hash_one((id, "seat")))
}

fn build_cut_message(id: PlayerId, replay: &Replay) -> ServerMessage {
    let world = replay.get_world();
    let (tick, _) = replay.find_cut().expect("only a cut world is announced");
    ServerMessage::Cut {
        id,
        tick,
        pose: build_pose(world),
        cut_lower_m: world.calc_cut_lower_m(),
        cake_touch_lower_m: world.calc_cake_touch_lower_m(),
        is_cut_through: world.is_cut_through(),
    }
}

/// Every cut, best first: a higher unrounded score ranks first, and of equal scores the earlier
/// cut. Cuts equal in both share a rank, as do the players who never touched the controls, since
/// their worlds ran alike to the same timeout cut.
#[expect(clippy::cast_possible_truncation, reason = "a room's players fit u32")]
fn rank(round: &Round) -> Vec<Standing> {
    let mut standings: Vec<Standing> = round
        .replays
        .iter()
        .map(|(&id, replay)| {
            let (tick, split) = replay.find_cut().expect("every replay is cut before the results");
            Standing {
                id,
                name: round.seats[&id].name.clone(),
                rank: 0,
                score: split.calc_score(),
                above_m3: split.above_m3,
                below_m3: split.below_m3,
                tick,
                has_touched: replay.has_touched(),
            }
        })
        .collect();
    let order = |a: &Standing, b: &Standing| b.score.total_cmp(&a.score).then(a.tick.cmp(&b.tick));
    standings.sort_by(order);
    for idx in 0..standings.len() {
        let is_tied = idx > 0 && order(&standings[idx - 1], &standings[idx]).is_eq();
        standings[idx].rank = if is_tied { standings[idx - 1].rank } else { idx as u32 + 1 };
    }
    standings
}

/// `standings` in the order the reveal judges them: every miss first, then the rest, each in a
/// random order.
fn build_judging_order(standings: &[Standing]) -> Vec<&Standing> {
    let mut order: Vec<&Standing> = standings.iter().collect();
    shuffle_randomly(&mut order);
    order.sort_by_key(|standing| !is_miss(standing));
    order
}

/// How long the judging counts `standing`'s grams: `max_count_ms` for a perfect cut, shorter by the
/// square of the score's share of a perfect one, so a near-perfect cut's count lingers and a poor
/// one's is quick, and none for a miss.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "a count of a few seconds in ms, from a share in 0-1"
)]
fn calc_count_ms(standing: &Standing, max_count_ms: u64) -> u64 {
    if is_miss(standing) {
        return 0;
    }
    let share = standing.score / MAX_SCORE;
    (max_count_ms as f64 * share * share).round() as u64
}

/// Whether `standing`'s knife missed the cake, which the core's cut scores as the whole cake above
/// the knife.
fn is_miss(standing: &Standing) -> bool {
    standing.below_m3 <= 0.0
}

/// Logs `notes` from the replay of `seat`, and tells its `player` of a desync if their page is connected.
fn report_notes(seat: &Seat, player: Option<&Player>, id: PlayerId, notes: &[Note]) {
    for note in notes {
        eprintln!("{} (#{id}): {note}", seat.name);
        if let (Note::Desync { tick }, Some(player)) = (*note, player) {
            send(player, &ServerMessage::Desync { tick });
        }
    }
}

fn broadcast(players: &BTreeMap<PlayerId, Player>, message: &ServerMessage) {
    let text = serde_json::to_string(message).expect("server messages serialize");
    for player in players.values() {
        send_frame(player, Frame::Text(text.clone()));
    }
}

fn send(player: &Player, message: &ServerMessage) {
    let text = serde_json::to_string(message).expect("server messages serialize");
    send_frame(player, Frame::Text(text));
}

fn send_frame(player: &Player, frame: Frame) {
    // A closed connection's leave event is on its way, and a full outbox is disconnected once the
    // event or step that filled it ends.
    let _ = player.outbox.try_send(frame);
}
