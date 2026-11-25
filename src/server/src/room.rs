//! The one game room a server holds: its players, host, pick, and round.
//!
//! Events from connections and [`Room::step`] at the simulation's tick rate advance it. It reads the time only from
//! its callers, so tests can drive it on a made-up clock.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{BuildHasher, RandomState};

use cake_core::input::Input;
use cake_core::sim::{ROUND_TICKS, TICKS_PER_S};
use rayon::prelude::*;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::oneshot;

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
/// Frames a connection may have waiting to send, about 8 s of pose frames, before the room
/// disconnects it as too slow to follow the round.
pub const OUTBOX_LEN: usize = 256;

/// One connection's identity, never reused. A connection plays as the player whose seat it
/// joined into, which a later connection can take over once this one has gone.
pub type ConnectionId = u32;

#[derive(Clone, Debug)]
pub struct RoomConfig {
    pub round_ticks: u32,
    /// How long a round waits for every player to load its cake before starting without them,
    /// who then spectate it.
    pub load_timeout_ms: u64,
    /// How far a connected player's world may trail the round's clock waiting for their inputs,
    /// past which it steps on with their last input, so the deadline, the round's length plus
    /// this, finds every world cut. A world whose page has gone keeps up with the clock instead.
    pub deadline_grace_ms: u64,
    /// How many ticks past the round's clock an input or cut may be stamped. One stamped further
    /// is refused, so no client can make one step replay its world far ahead of real time.
    pub max_lead_ticks: u32,
    /// How long a player whose page closed keeps their seat past their last activity, for the page
    /// to reopen into.
    pub seat_hold_ms: u64,
    /// How long the judging waits after the round's last cut, for every page's opponent views to
    /// play it: the 2.2 s knife motion plus their playback lag.
    pub judging_delay_ms: u64,
    /// How long the judging reveal shows each round player, one after another, before the results.
    pub judging_scene_ms: u64,
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
            max_lead_ticks: TICKS_PER_S / 2,
            seat_hold_ms: 60 * 60 * 1000,
            judging_delay_ms: 3_000,
            judging_scene_ms: 5_000,
            judging_tail_ms: 1_500,
        }
    }
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
    /// A new connection, which plays as a fresh player of the connection's own id unless its
    /// `seat` token reopens a seat whose page closed, and is told on `assigned` which id it plays as.
    Join {
        connection: ConnectionId,
        name: String,
        standup_preference: f64,
        seat: Option<String>,
        outbox: Outbox,
        assigned: oneshot::Sender<PlayerId>,
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
            | Self::Hash { connection, .. } => connection,
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
    outbox: Outbox,
}

#[derive(Clone)]
struct Seat {
    name: String,
    /// From 0, first to give their standup update, to 1, last. No one else sees it before the results.
    standup_preference: f64,
}

/// A seat whose page closed, held for it to reopen into.
struct HeldSeat {
    token: String,
    last_active_ms: u64,
}

/// The next round's cake, always a catalog id, so every page can download it in the lobby, and
/// whether the host picked Random, which settled on it at once and is never rerolled at the start.
struct Pick {
    cake_id: String,
    is_random: bool,
}

/// A round's players stay in it to the end, judged at the deadline if their page closed.
struct Round {
    cake_id: String,
    replays: BTreeMap<PlayerId, Replay>,
    /// Each round player as they started, so a player whose page closed is still named and seated.
    seats: BTreeMap<PlayerId, Seat>,
    /// Round players whose cut everyone has been told of.
    announced_cuts: BTreeSet<PlayerId>,
}

enum Phase {
    Lobby,
    Loading {
        round: Round,
        ready: BTreeSet<PlayerId>,
        since_ms: u64,
    },
    Playing {
        round: Round,
        start_ms: u64,
        steps_since_poses: u32,
        /// Once every player has cut, when the judging starts.
        judging_at_ms: Option<u64>,
    },
    /// The reveal every page shows in step, judging each player in `order` for a scene apiece.
    Judging {
        round: Round,
        standings: Vec<Standing>,
        order: Vec<PlayerId>,
        start_ms: u64,
    },
    /// Until the host returns everyone to the lobby.
    Results {
        round: Round,
        standings: Vec<Standing>,
        standup: Vec<Member>,
    },
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
        }
    }

    pub fn on_event(&mut self, event: RoomEvent, now_ms: u64) {
        match event {
            RoomEvent::Join { connection, name, standup_preference, seat, outbox, assigned } => {
                let id = self.join(connection, Seat { name, standup_preference }, seat.as_deref(), outbox, now_ms);
                // The connection may have closed meanwhile; its leave event follows.
                let _ = assigned.send(id);
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
            RoomEvent::Ready { .. } => {
                if let Phase::Loading { round, ready, .. } = &mut self.phase
                    && round.replays.contains_key(&id)
                {
                    ready.insert(id);
                }
            }
            RoomEvent::Input { tick, input, .. } => {
                // A page sends an input every tick of play, so only one that changed is the player's own doing.
                if let Some(replay) = self.find_replay_within_lead(id, "input", tick, now_ms) {
                    let is_changed = replay.find_last_pushed_input() != Some(input);
                    replay.push_input(tick, input);
                    if is_changed {
                        self.touch(id, now_ms);
                    }
                }
            }
            RoomEvent::Cut { tick, .. } => {
                if let Some(replay) = self.find_replay_within_lead(id, "cut", tick, now_ms) {
                    replay.push_cut(tick);
                }
                self.touch(id, now_ms);
            }
            RoomEvent::Hash { tick, hash, .. } => {
                if let Some(replay) = self.find_replay(id) {
                    replay.push_hash(tick, hash);
                }
            }
        }
    }

    /// Advances the round one tick of wall time: replays what clients sent, holds each world to
    /// the round's clock, streams poses, and moves between phases. Like every event, it ends by
    /// disconnecting any player whose outbox filled.
    pub fn step(&mut self, now_ms: u64) {
        self.step_phase(now_ms);
        self.expire_held_seats(now_ms);
        self.disconnect_overflowed();
    }

    fn find_player_id(&self, connection: ConnectionId) -> Option<PlayerId> {
        self.players.iter().find(|(_, player)| player.connection == connection).map(|(&id, _)| id)
    }

    fn expire_held_seats(&mut self, now_ms: u64) {
        let hold_ms = self.config.seat_hold_ms;
        self.held_seats.retain(|id, held| {
            let is_kept = now_ms.saturating_sub(held.last_active_ms) <= hold_ms;
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

    fn step_phase(&mut self, now_ms: u64) {
        match &mut self.phase {
            Phase::Lobby | Phase::Results { .. } => {}
            Phase::Loading { round, ready, since_ms } => {
                let is_all_ready = round.replays.keys().all(|id| ready.contains(id));
                if is_all_ready || now_ms >= *since_ms + self.config.load_timeout_ms {
                    self.start_playing(now_ms);
                }
            }
            Phase::Playing { .. } => self.step_playing(now_ms),
            Phase::Judging { .. } => self.step_judging(now_ms),
        }
    }

    /// Seats `connection` as a fresh player of the same id, or in the held seat its `token`
    /// names, and returns the id it plays as. A seat still connected elsewhere, or expired, is
    /// not reopened.
    fn join(
        &mut self,
        connection: ConnectionId,
        seat: Seat,
        token: Option<&str>,
        outbox: Outbox,
        now_ms: u64,
    ) -> PlayerId {
        let held_id = token.and_then(|token| self.find_held_seat(token));
        let (id, token) = match held_id {
            Some(id) => (id, self.held_seats.remove(&id).expect("a found seat is held").token),
            None => (connection, create_seat_token(connection)),
        };
        eprintln!("{} (#{id}) {}", seat.name, if held_id.is_some() { "rejoined" } else { "joined" });
        // The page left while the round loaded, which marked it ready, but its reopened page must
        // load the cake again within the limit, as any page does.
        if let Phase::Loading { ready, .. } = &mut self.phase {
            ready.remove(&id);
        }
        let player = Player { connection, seat, token, last_active_ms: now_ms, outbox };
        send(&player, &ServerMessage::Welcome { id, seat: player.token.clone() });
        self.players.insert(id, player);
        self.broadcast_room();
        self.send_round_state(id, now_ms);
        id
    }

    fn find_held_seat(&self, token: &str) -> Option<PlayerId> {
        self.held_seats.iter().find(|(_, held)| held.token == token).map(|(&id, _)| id)
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
            Phase::Playing { round, start_ms, .. } => (round, Some(*start_ms)),
            Phase::Loading { round, .. } | Phase::Judging { round, .. } | Phase::Results { round, .. } => (round, None),
        };
        send(player, &self.build_round_message(round));
        if let Some(start_ms) = started_ms {
            send(player, &ServerMessage::Start { elapsed_ms: now_ms - start_ms });
        }
        for &cut_id in &round.announced_cuts {
            send(player, &build_cut_message(cut_id, &round.replays[&cut_id]));
        }
        if let Some(replay) = round.replays.get(&id) {
            send(player, &build_resume_message(replay));
        }
        match &self.phase {
            Phase::Judging { standings, order, start_ms, .. } => {
                send(player, &self.build_judging_message(standings, order, now_ms - start_ms));
            }
            Phase::Results { standings, standup, .. } => {
                send(player, &ServerMessage::Results { standings: standings.clone(), standup: standup.clone() });
            }
            _ => {}
        }
    }

    /// Holds the seat of a player whose page closed. A round they are in plays on without them:
    /// their world lets go of the cake and knife, and keeps up with the round's clock to its cut.
    fn leave(&mut self, id: PlayerId) {
        let Some(player) = self.players.remove(&id) else {
            return;
        };
        eprintln!("{} (#{id}) left", player.seat.name);
        self.held_seats.insert(id, HeldSeat { token: player.token, last_active_ms: player.last_active_ms });
        if self.host == Some(id) {
            self.host = None;
        }
        match &mut self.phase {
            Phase::Lobby | Phase::Judging { .. } | Phase::Results { .. } => {}
            Phase::Loading { round, ready, .. } => {
                // Nothing waits on a page that has gone, and its world needs no release before it
                // has stepped, which would take the place of a reopened page's first input.
                if round.replays.contains_key(&id) {
                    ready.insert(id);
                }
            }
            Phase::Playing { round, .. } => {
                if let Some(replay) = round.replays.get_mut(&id) {
                    replay.push_release();
                }
            }
        }
        // A reveal or results no one is left to see would greet the next joiner, so the room moves on.
        if self.players.is_empty() && matches!(self.phase, Phase::Judging { .. } | Phase::Results { .. }) {
            self.enter_lobby();
            return;
        }
        self.broadcast_room();
    }

    /// Makes player `id` host if `password` is the host password, outside a round.
    fn claim_host(&mut self, id: PlayerId, password: &str) {
        let player = &self.players[&id];
        if matches!(self.phase, Phase::Loading { .. } | Phase::Playing { .. } | Phase::Judging { .. }) {
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
        let model = self.catalog.find(&cake_id).expect("the pick is a catalog cake");
        let round_ticks = self.config.round_ticks;
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        let replays = ids.into_par_iter().map(|id| (id, Replay::create(model.clone(), round_ticks))).collect();
        eprintln!("round on {cake_id} for {} players", self.players.len());
        let seats = self.players.iter().map(|(&id, player)| (id, player.seat.clone())).collect();
        let round = Round { cake_id, replays, seats, announced_cuts: BTreeSet::new() };
        let message = self.build_round_message(&round);
        self.phase = Phase::Loading { round, ready: BTreeSet::new(), since_ms: now_ms };
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

    /// Starts the round on time: a page that has not loaded the cake by now sits it out as a
    /// spectator, and a round no page loaded is called off.
    fn start_playing(&mut self, now_ms: u64) {
        let Phase::Loading { mut round, ready, since_ms } = std::mem::replace(&mut self.phase, Phase::Lobby) else {
            unreachable!("only a loading round starts");
        };
        eprintln!(
            "round started {} ms after loading, {} of {} players ready",
            now_ms - since_ms,
            ready.len(),
            round.replays.len()
        );
        let late: Vec<PlayerId> = round.replays.keys().filter(|id| !ready.contains(id)).copied().collect();
        for id in &late {
            round.replays.remove(id);
            let seat = round.seats.remove(id).expect("every round player has a seat");
            eprintln!("{} (#{id}) sits the round out, the cake not loaded", seat.name);
        }
        if !late.is_empty() {
            self.broadcast(&ServerMessage::SitOut { players: late });
        }
        if round.replays.is_empty() {
            eprintln!("round called off, since no player loaded the cake");
            self.enter_lobby();
            return;
        }
        self.phase = Phase::Playing { round, start_ms: now_ms, steps_since_poses: 0, judging_at_ms: None };
        self.broadcast_room();
        self.broadcast(&ServerMessage::Start { elapsed_ms: 0 });
    }

    fn step_playing(&mut self, now_ms: u64) {
        let lag_ticks = calc_ticks(self.config.deadline_grace_ms);
        let judging_delay_ms = self.config.judging_delay_ms;
        let Phase::Playing { round, start_ms, steps_since_poses, judging_at_ms } = &mut self.phase else {
            unreachable!("only a round in play steps");
        };
        let clock_tick = calc_ticks(now_ms.saturating_sub(*start_ms));
        let players = &self.players;
        // Each world is independent and single-threaded, so the worlds step in parallel: one
        // world's tick with the knife lowered on a detailed cake can take most of the budget.
        let notes: Vec<(PlayerId, Vec<Note>)> = round
            .replays
            .par_iter_mut()
            .map(|(&id, replay)| {
                let notes = replay.drain();
                // A page that has gone sends nothing more, so its world keeps up with the clock and
                // cuts as time runs out, where a connected page's inputs get the grace to arrive.
                let lag = if players.contains_key(&id) { lag_ticks } else { 0 };
                replay.hold_to(clock_tick.saturating_sub(lag));
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
            *judging_at_ms = Some(now_ms + judging_delay_ms);
            broadcast(players, &ServerMessage::PlayOver);
        }
        if judging_at_ms.is_some_and(|at_ms| now_ms >= at_ms) {
            self.start_judging(now_ms);
            return;
        }
        *steps_since_poses += 1;
        if *steps_since_poses >= POSE_INTERVAL_STEPS {
            *steps_since_poses = 0;
            broadcast_poses(round, players);
        }
    }

    /// Ranks every cut and starts the reveal that judges each player in a random order, which
    /// every page shows in step, then the results once its last scene and tail have played.
    fn start_judging(&mut self, now_ms: u64) {
        let Phase::Playing { round, .. } = std::mem::replace(&mut self.phase, Phase::Lobby) else {
            unreachable!("only a round in play is judged");
        };
        let standings = rank(&round);
        for standing in &standings {
            eprintln!(
                "rank {}: {} (#{}) {} at tick {}",
                standing.rank, standing.name, standing.id, standing.score, standing.tick
            );
        }
        let mut order: Vec<PlayerId> = round.replays.keys().copied().collect();
        shuffle_randomly(&mut order);
        let names: Vec<&str> = order.iter().map(|id| round.seats[id].name.as_str()).collect();
        eprintln!(
            "judging order: {}, {} ms a scene, results in {} ms",
            names.join(", "),
            self.config.judging_scene_ms,
            self.calc_judging_ms(order.len())
        );
        // A round everyone's page left plays out to its judging, which no one is left to see.
        if self.players.is_empty() {
            self.enter_lobby();
            return;
        }
        let message = self.build_judging_message(&standings, &order, 0);
        self.phase = Phase::Judging { round, standings, order, start_ms: now_ms };
        self.broadcast_room();
        self.broadcast(&message);
        self.step_judging(now_ms);
    }

    /// How long the judging reveal of `player_len` players lasts, tail included.
    const fn calc_judging_ms(&self, player_len: usize) -> u64 {
        player_len as u64 * self.config.judging_scene_ms + self.config.judging_tail_ms
    }

    fn build_judging_message(&self, standings: &[Standing], order: &[PlayerId], elapsed_ms: u64) -> ServerMessage {
        ServerMessage::Judging {
            order: order.to_vec(),
            scene_ms: self.config.judging_scene_ms,
            elapsed_ms,
            standings: standings.to_vec(),
        }
    }

    /// Shows the results once the judging reveal has played to its end.
    fn step_judging(&mut self, now_ms: u64) {
        let Phase::Judging { order, start_ms, .. } = &self.phase else {
            unreachable!("only a round being judged steps its reveal");
        };
        if now_ms >= start_ms + self.calc_judging_ms(order.len()) {
            self.show_results();
        }
    }

    fn show_results(&mut self) {
        let Phase::Judging { round, standings, .. } = std::mem::replace(&mut self.phase, Phase::Lobby) else {
            unreachable!("only a judged round ends");
        };
        let standup = self.build_standup(&round, &standings);
        let names: Vec<&str> = standup.iter().map(|member| member.name.as_str()).collect();
        eprintln!("standup order: {}", names.join(", "));
        let message = ServerMessage::Results { standings: standings.clone(), standup: standup.clone() };
        self.phase = Phase::Results { round, standings, standup };
        self.broadcast_room();
        self.broadcast(&message);
    }

    /// The Standup Order of everyone in the round or the room: the round's players by their
    /// `standings`, those whose page closed included, then the spectators in join order, which
    /// their ids follow.
    fn build_standup(&self, round: &Round, standings: &[Standing]) -> Vec<Member> {
        let ranked: Vec<(PlayerId, u32)> = standings.iter().map(|standing| (standing.id, standing.rank)).collect();
        let spectators: Vec<PlayerId> =
            self.players.keys().filter(|id| !round.replays.contains_key(id)).copied().collect();
        let find_seat = |id: PlayerId| round.seats.get(&id).unwrap_or_else(|| &self.players[&id].seat);
        let choosers: Vec<Chooser> = order_choosers(&ranked, &spectators, shuffle_randomly)
            .into_iter()
            .map(|id| Chooser { id, preference: find_seat(id).standup_preference })
            .collect();
        calc_standup_order(&choosers).into_iter().map(|id| Member { id, name: find_seat(id).name.clone() }).collect()
    }

    /// Player `id`'s replay, while the round is in play.
    fn find_replay(&mut self, id: PlayerId) -> Option<&mut Replay> {
        match &mut self.phase {
            Phase::Playing { round, .. } => round.replays.get_mut(&id),
            _ => None,
        }
    }

    /// [`Room::find_replay`], unless the `kind` of message they stamped for `tick` is further
    /// ahead of the round's clock than the lead allows, which is refused and noted for the next
    /// step's log, one line however many arrive.
    fn find_replay_within_lead(
        &mut self,
        id: PlayerId,
        kind: &'static str,
        tick: u32,
        now_ms: u64,
    ) -> Option<&mut Replay> {
        let Phase::Playing { round, start_ms, .. } = &mut self.phase else {
            return None;
        };
        let replay = round.replays.get_mut(&id)?;
        let clock_tick = calc_ticks(now_ms.saturating_sub(*start_ms));
        if u64::from(tick) > u64::from(clock_tick) + u64::from(self.config.max_lead_ticks) {
            replay.note_refused(kind, tick, clock_tick);
            return None;
        }
        Some(replay)
    }

    /// Disconnects every player whose outbox has filled, since their connection is not keeping up.
    fn disconnect_overflowed(&mut self) {
        // Each leave tells everyone left, which may fill another outbox.
        while let Some(id) = self.players.iter().find(|(_, player)| player.outbox.capacity() == 0).map(|(&id, _)| id) {
            eprintln!("#{id}: {OUTBOX_LEN} frames unsent, disconnecting");
            self.leave(id);
        }
    }

    fn build_round_message(&self, round: &Round) -> ServerMessage {
        ServerMessage::Round {
            cake: round.cake_id.clone(),
            round_ticks: self.config.round_ticks,
            players: round.seats.iter().map(|(&id, seat)| Member { id, name: seat.name.clone() }).collect(),
        }
    }

    fn build_pick_message(&self) -> ServerMessage {
        ServerMessage::Pick { cake: self.pick.cake_id.clone(), is_random: self.pick.is_random }
    }

    fn broadcast_room(&self) {
        let phase = match self.phase {
            Phase::Lobby => PhaseName::Lobby,
            Phase::Loading { .. } => PhaseName::Loading,
            Phase::Playing { .. } => PhaseName::Playing,
            Phase::Judging { .. } => PhaseName::Judging,
            Phase::Results { .. } => PhaseName::Results,
        };
        let members = self.players.iter().map(|(&id, player)| Member { id, name: player.seat.name.clone() }).collect();
        self.broadcast(&ServerMessage::Room { phase, host: self.host, members });
    }

    fn broadcast(&self, message: &ServerMessage) {
        broadcast(&self.players, message);
    }
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
