//! The one game room a server holds: its players, host, and round, advanced by events from
//! connections and by [`Room::step`] at the simulation's tick rate. It reads the time only from
//! its callers, so tests can drive it on a made-up clock.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cake_core::input::Input;
use cake_core::sim::{Cake, ROUND_TICKS, TICKS_PER_S};
use rayon::prelude::*;
use tokio::sync::mpsc::{Receiver, Sender, channel};

use crate::catalog::Catalog;
use crate::protocol::{
    Member, POSE_RECORD_SIZE, PhaseName, PlayerId, ServerMessage, Standing, build_pose, encode_pose_record,
};
use crate::replay::{Note, Replay};

/// Steps between pose frames, which stream every round player's world to every client.
const POSE_INTERVAL_STEPS: u32 = 2;
/// Frames a connection may have waiting to send, about 8 s of pose frames, before the room
/// disconnects it as too slow to follow the round.
pub const OUTBOX_LEN: usize = 256;

#[derive(Clone, Debug)]
pub struct RoomConfig {
    pub round_ticks: u32,
    /// How long a round waits for every player to load its cake before starting without them.
    pub load_timeout_ms: u64,
    /// How long past the round's length the server waits for late inputs before cutting for
    /// every player who has not cut.
    pub deadline_grace_ms: u64,
    /// How long the results show before the room returns to the lobby.
    pub results_hold_ms: u64,
    /// How many ticks past the round's clock an input or cut may be stamped. One stamped further
    /// is refused, so no client can make one step replay its world far ahead of real time.
    pub max_lead_ticks: u32,
}

impl Default for RoomConfig {
    fn default() -> Self {
        RoomConfig {
            round_ticks: ROUND_TICKS,
            load_timeout_ms: 10_000,
            deadline_grace_ms: 2_000,
            results_hold_ms: 10_000,
            max_lead_ticks: TICKS_PER_S / 2,
        }
    }
}

/// A frame for one connection to send.
#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
}

/// Where the room puts a connection's frames, holding at most [`OUTBOX_LEN`].
pub type Outbox = Sender<Frame>;

/// A connection's outbox and the end its connection sends from.
pub fn create_outbox() -> (Outbox, Receiver<Frame>) {
    channel(OUTBOX_LEN)
}

/// What a connection tells the room, already validated.
pub enum RoomEvent {
    Join { id: PlayerId, name: String, outbox: Outbox },
    Leave { id: PlayerId },
    Start { id: PlayerId, cake: String },
    Ready { id: PlayerId },
    Input { id: PlayerId, tick: u32, input: Input },
    Cut { id: PlayerId, tick: u32 },
    Hash { id: PlayerId, tick: u32, hash: u64 },
}

struct Player {
    name: String,
    outbox: Outbox,
}

struct Round {
    cake_id: String,
    replays: BTreeMap<PlayerId, Replay>,
    /// Round players whose cut everyone has been told of, each still in `replays`.
    announced_cuts: BTreeSet<PlayerId>,
}

impl Round {
    fn remove_player(&mut self, id: PlayerId) {
        self.replays.remove(&id);
        self.announced_cuts.remove(&id);
    }
}

enum Phase {
    Lobby,
    Loading { round: Round, ready: BTreeSet<PlayerId>, since_ms: u64 },
    Playing { round: Round, start_ms: u64, steps_since_poses: u32 },
    Results { round: Round, standings: Vec<Standing>, since_ms: u64 },
}

pub struct Room {
    config: RoomConfig,
    catalog: Catalog,
    players: BTreeMap<PlayerId, Player>,
    host: Option<PlayerId>,
    phase: Phase,
}

impl Room {
    pub fn create(config: RoomConfig, catalog: Catalog) -> Room {
        Room { config, catalog, players: BTreeMap::new(), host: None, phase: Phase::Lobby }
    }

    pub fn on_event(&mut self, event: RoomEvent, now_ms: u64) {
        match event {
            RoomEvent::Join { id, name, outbox } => self.join(id, name, outbox, now_ms),
            RoomEvent::Leave { id } => self.leave(id),
            RoomEvent::Start { id, cake } => self.start_loading(id, &cake, now_ms),
            RoomEvent::Ready { id } => {
                if let Phase::Loading { round, ready, .. } = &mut self.phase
                    && round.replays.contains_key(&id)
                {
                    ready.insert(id);
                }
            }
            RoomEvent::Input { id, tick, input } => {
                if let Some(replay) = self.find_replay_within_lead(id, "input", tick, now_ms) {
                    replay.push_input(tick, input);
                }
            }
            RoomEvent::Cut { id, tick } => {
                if let Some(replay) = self.find_replay_within_lead(id, "cut", tick, now_ms) {
                    replay.push_cut(tick);
                }
            }
            RoomEvent::Hash { id, tick, hash } => {
                if let Some(replay) = self.find_replay(id) {
                    replay.push_hash(tick, hash);
                }
            }
        }
        self.disconnect_overflowed();
    }

    /// Advances the round one tick of wall time: replays what clients sent, cuts for everyone at
    /// the deadline, streams poses, and moves between phases. Like every event, it ends by
    /// disconnecting any player whose outbox filled.
    pub fn step(&mut self, now_ms: u64) {
        self.step_phase(now_ms);
        self.disconnect_overflowed();
    }

    fn step_phase(&mut self, now_ms: u64) {
        match &mut self.phase {
            Phase::Lobby => {}
            Phase::Loading { round, ready, since_ms } => {
                let is_all_ready = round.replays.keys().all(|id| ready.contains(id));
                if is_all_ready || now_ms >= *since_ms + self.config.load_timeout_ms {
                    self.start_playing(now_ms);
                }
            }
            Phase::Playing { .. } => self.step_playing(now_ms),
            Phase::Results { since_ms, .. } => {
                if now_ms >= *since_ms + self.config.results_hold_ms {
                    self.phase = Phase::Lobby;
                    self.broadcast_room();
                }
            }
        }
    }

    fn join(&mut self, id: PlayerId, name: String, outbox: Outbox, now_ms: u64) {
        eprintln!("{name} (#{id}) joined");
        let player = Player { name, outbox };
        send(&player, &ServerMessage::Welcome { id });
        self.players.insert(id, player);
        self.host.get_or_insert(id);
        self.broadcast_room();
        let player = &self.players[&id];
        let (round, started_ms) = match &self.phase {
            Phase::Lobby => return,
            Phase::Loading { round, .. } => (round, None),
            Phase::Playing { round, start_ms, .. } => (round, Some(*start_ms)),
            Phase::Results { round, .. } => (round, None),
        };
        send(player, &self.build_round_message(round));
        if let Some(start_ms) = started_ms {
            send(player, &ServerMessage::Start { elapsed_ms: now_ms - start_ms });
        }
        for &cut_id in &round.announced_cuts {
            send(player, &build_cut_message(cut_id, &round.replays[&cut_id]));
        }
        if let Phase::Results { standings, .. } = &self.phase {
            send(player, &ServerMessage::Results { standings: standings.clone() });
        }
    }

    fn leave(&mut self, id: PlayerId) {
        let Some(player) = self.players.remove(&id) else {
            return;
        };
        eprintln!("{} (#{id}) left", player.name);
        if self.host == Some(id) {
            self.host = self.players.keys().next().copied();
        }
        match &mut self.phase {
            Phase::Lobby => {}
            Phase::Loading { round, ready, .. } => {
                round.remove_player(id);
                ready.remove(&id);
            }
            Phase::Playing { round, .. } => round.remove_player(id),
            // The results stand as ranked, so a late joiner still gets the leaver's cut and rank.
            Phase::Results { .. } => {}
        }
        if self.players.is_empty() {
            self.phase = Phase::Lobby;
        }
        self.broadcast_room();
    }

    fn start_loading(&mut self, id: PlayerId, cake: &str, now_ms: u64) {
        // A connection the room disconnected may still have messages on their way.
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let (cake_id, model) = match self.validate_start(id, cake) {
            Ok(found) => found,
            Err(message) => {
                send(player, &ServerMessage::Error { message });
                return;
            }
        };
        let round_ticks = self.config.round_ticks;
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        let replays = ids.into_par_iter().map(|id| (id, Replay::create(model.clone(), round_ticks))).collect();
        let round = Round { cake_id: cake_id.to_string(), replays, announced_cuts: BTreeSet::new() };
        eprintln!("round on {cake_id} for {} players", round.replays.len());
        let message = self.build_round_message(&round);
        self.phase = Phase::Loading { round, ready: BTreeSet::new(), since_ms: now_ms };
        self.broadcast_room();
        self.broadcast(&message);
    }

    /// The catalog's id and model for the `cake` player `id` asks to start a round on, or why
    /// they may not.
    fn validate_start(&self, id: PlayerId, cake: &str) -> Result<(&str, &Arc<Cake>), String> {
        if self.host != Some(id) {
            return Err("only the host can start a round".to_string());
        }
        if !matches!(self.phase, Phase::Lobby) {
            return Err("a round is already on".to_string());
        }
        self.catalog.find(cake).ok_or_else(|| format!("no cake {cake:?}"))
    }

    fn start_playing(&mut self, now_ms: u64) {
        let Phase::Loading { round, .. } = std::mem::replace(&mut self.phase, Phase::Lobby) else {
            unreachable!("only a loading round starts");
        };
        self.phase = Phase::Playing { round, start_ms: now_ms, steps_since_poses: 0 };
        self.broadcast_room();
        self.broadcast(&ServerMessage::Start { elapsed_ms: 0 });
    }

    fn step_playing(&mut self, now_ms: u64) {
        let round_ms = self.config.round_ticks as u64 * 1000 / TICKS_PER_S as u64;
        let Phase::Playing { round, start_ms, steps_since_poses } = &mut self.phase else {
            unreachable!("only a round in play steps");
        };
        let is_past_deadline = now_ms >= *start_ms + round_ms + self.config.deadline_grace_ms;
        // Each world is independent and single-threaded, so the worlds step in parallel: one
        // world's tick with the knife lowered on a detailed cake can take most of the budget.
        let notes: Vec<(PlayerId, Vec<Note>)> = round
            .replays
            .par_iter_mut()
            .map(|(&id, replay)| (id, if is_past_deadline { replay.finish() } else { replay.drain() }))
            .collect();
        for (id, notes) in notes {
            report_notes(&self.players[&id], id, &notes);
        }
        announce_new_cuts(round, &self.players);
        if round.replays.values().all(Replay::is_cut) {
            self.show_results(now_ms);
            return;
        }
        *steps_since_poses += 1;
        if *steps_since_poses >= POSE_INTERVAL_STEPS {
            *steps_since_poses = 0;
            broadcast_poses(round, &self.players);
        }
    }

    fn show_results(&mut self, now_ms: u64) {
        let Phase::Playing { round, .. } = std::mem::replace(&mut self.phase, Phase::Lobby) else {
            unreachable!("only a round in play ends");
        };
        let standings = rank(&round, &self.players);
        for standing in &standings {
            eprintln!(
                "rank {}: {} (#{}) {} at tick {}",
                standing.rank, standing.name, standing.id, standing.score, standing.tick
            );
        }
        let message = ServerMessage::Results { standings: standings.clone() };
        self.phase = Phase::Results { round, standings, since_ms: now_ms };
        self.broadcast_room();
        self.broadcast(&message);
    }

    fn find_replay(&mut self, id: PlayerId) -> Option<&mut Replay> {
        match &mut self.phase {
            Phase::Playing { round, .. } => round.replays.get_mut(&id),
            _ => None,
        }
    }

    /// Player `id`'s replay in play, unless the `kind` of message they stamped for `tick` is
    /// further ahead of the round's clock than the lead allows, which is logged and refused.
    fn find_replay_within_lead(&mut self, id: PlayerId, kind: &str, tick: u32, now_ms: u64) -> Option<&mut Replay> {
        let Phase::Playing { round, start_ms, .. } = &mut self.phase else {
            return None;
        };
        let replay = round.replays.get_mut(&id)?;
        let round_tick = now_ms.saturating_sub(*start_ms) * TICKS_PER_S as u64 / 1000;
        if tick as u64 > round_tick + self.config.max_lead_ticks as u64 {
            eprintln!(
                "{} (#{id}): {kind} stamped tick {tick} refused at round tick {round_tick}",
                self.players[&id].name
            );
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
            players: round.replays.keys().copied().collect(),
        }
    }

    fn broadcast_room(&self) {
        let phase = match self.phase {
            Phase::Lobby => PhaseName::Lobby,
            Phase::Loading { .. } => PhaseName::Loading,
            Phase::Playing { .. } => PhaseName::Playing,
            Phase::Results { .. } => PhaseName::Results,
        };
        let members = self.players.iter().map(|(&id, player)| Member { id, name: player.name.clone() }).collect();
        self.broadcast(&ServerMessage::Room { phase, host: self.host, members });
    }

    fn broadcast(&self, message: &ServerMessage) {
        broadcast(&self.players, message);
    }
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
        eprintln!("{} (#{id}) cut at tick {tick}: score {}", players[&id].name, split.calc_score());
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
fn rank(round: &Round, players: &BTreeMap<PlayerId, Player>) -> Vec<Standing> {
    let mut standings: Vec<Standing> = round
        .replays
        .iter()
        .map(|(&id, replay)| {
            let (tick, split) = replay.find_cut().expect("every replay is cut before the results");
            Standing {
                id,
                name: players[&id].name.clone(),
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

fn report_notes(player: &Player, id: PlayerId, notes: &[Note]) {
    for note in notes {
        eprintln!("{} (#{id}): {note}", player.name);
        if let Note::Desync { tick } = *note {
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
