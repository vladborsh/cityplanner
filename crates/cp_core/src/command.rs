//! Generic command bus (`architecture/system-design.md` §4, level 1). No domain types:
//! the command enum `C` is supplied by the caller (`SimCommand` in `cp_sim`, a test enum
//! in tests).

use std::fmt;

use bevy_ecs::message::Message;
use bevy_ecs::resource::Resource;
use serde::{Deserialize, Serialize};

use crate::time::Tick;

/// Sequence number issued by a [`CommandQueue`]; strictly increasing, never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CommandId(pub u64);

/// Money in integer cents. Signed so refunds and balances share one type.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Cost(pub i64);

impl Cost {
    pub const ZERO: Cost = Cost(0);

    /// `units` of currency (how content RON writes money, `cost_per_m: 12.5`) as cents:
    /// times 100, rounded half away from zero.
    pub fn from_units(units: f32) -> Cost {
        Cost::scaled(units, 1.0)
    }

    /// `units × factor` of currency as cents (`architecture/system-design.md` §11). The
    /// product is taken in `f64` and rounded **once**, half away from zero; callers pass the
    /// whole transaction's factor (length, quantity, percent) rather than summing rounded
    /// parts.
    pub fn scaled(units: f32, factor: f64) -> Cost {
        Cost((f64::from(units) * factor * 100.0).round() as i64)
    }
}

impl std::ops::Add for Cost {
    type Output = Cost;
    fn add(self, rhs: Cost) -> Cost {
        Cost(self.0 + rhs.0)
    }
}

impl std::ops::AddAssign for Cost {
    fn add_assign(&mut self, rhs: Cost) {
        self.0 += rhs.0;
    }
}

impl std::ops::Sub for Cost {
    type Output = Cost;
    fn sub(self, rhs: Cost) -> Cost {
        Cost(self.0 - rhs.0)
    }
}

impl std::iter::Sum for Cost {
    fn sum<I: Iterator<Item = Cost>>(iter: I) -> Cost {
        Cost(iter.map(|c| c.0).sum())
    }
}

/// Why a command was not applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    InsufficientFunds {
        needed: Cost,
        available: Cost,
    },
    TileNotOwned,
    /// A domain wraps its own validation error. Typed errors for previews come straight
    /// from the domain's pure validation functions, not from this outcome.
    Rejected {
        domain: &'static str,
        reason: String,
    },
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsufficientFunds { needed, available } => {
                write!(
                    f,
                    "insufficient funds: need {}, have {}",
                    needed.0, available.0
                )
            }
            Self::TileNotOwned => write!(f, "tile not owned"),
            Self::Rejected { domain, reason } => write!(f, "{domain}: {reason}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Result of executing one command, emitted as a message in the `Commands` phase.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct CommandOutcome {
    pub id: CommandId,
    pub result: Result<Cost, CommandError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Queued<C> {
    tick: Tick,
    id: CommandId,
    command: C,
}

/// Commands waiting for their execution tick. Kept sorted by `(tick, CommandId)`.
#[derive(Resource, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandQueue<C: Send + Sync + 'static> {
    next_id: u64,
    pending: Vec<Queued<C>>,
}

impl<C: Send + Sync + 'static> Default for CommandQueue<C> {
    fn default() -> Self {
        Self {
            next_id: 0,
            pending: Vec::new(),
        }
    }
}

impl<C: Send + Sync + 'static> CommandQueue<C> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Schedules `command` to run at `tick`. Ids are issued in call order, so within one
    /// tick commands run in the order they were pushed.
    pub fn push(&mut self, command: C, tick: Tick) -> CommandId {
        let id = CommandId(self.next_id);
        self.next_id += 1;
        let at = self
            .pending
            .partition_point(|q| (q.tick, q.id) <= (tick, id));
        self.pending.insert(at, Queued { tick, id, command });
        id
    }

    /// Removes and returns every command due at or before `tick`, ordered by
    /// `(execution tick, CommandId)`.
    pub fn drain_due(&mut self, tick: Tick) -> Vec<(Tick, CommandId, C)> {
        let n = self.pending.partition_point(|q| q.tick <= tick);
        self.pending
            .drain(..n)
            .map(|q| (q.tick, q.id, q.command))
            .collect()
    }

    /// Applies `f` to every waiting command (for load-time id remapping; the order and ids of
    /// the queue are untouched).
    pub fn map_commands(&mut self, mut f: impl FnMut(&mut C)) {
        for q in &mut self.pending {
            f(&mut q.command);
        }
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// One applied command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry<C> {
    /// Tick in which the command was applied.
    pub tick: Tick,
    pub id: CommandId,
    pub command: C,
}

/// Applied commands in application order. Saved with the game; replay = save + log.
#[derive(Resource, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandLog<C: Send + Sync + 'static> {
    entries: Vec<LogEntry<C>>,
}

impl<C: Send + Sync + 'static> Default for CommandLog<C> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<C: Send + Sync + 'static> CommandLog<C> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, tick: Tick, id: CommandId, command: C) {
        self.entries.push(LogEntry { tick, id, command });
    }

    pub fn entries(&self) -> &[LogEntry<C>] {
        &self.entries
    }

    /// Applies `f` to every logged command (for load-time id remapping).
    pub fn map_commands(&mut self, mut f: impl FnMut(&mut C)) {
        for e in &mut self.entries {
            f(&mut e.command);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    enum TestCmd {
        A(u32),
        B { x: i8 },
    }

    #[test]
    fn drains_by_tick_then_arrival_order() {
        let mut q = CommandQueue::new();
        let t5a = q.push(TestCmd::A(1), Tick(5));
        let t3a = q.push(TestCmd::A(2), Tick(3));
        let t5b = q.push(TestCmd::B { x: 3 }, Tick(5));
        let t3b = q.push(TestCmd::A(4), Tick(3));
        assert!(t5a < t3a && t3a < t5b && t5b < t3b); // ids follow call order

        assert!(q.drain_due(Tick(2)).is_empty());
        let due = q.drain_due(Tick(5));
        let order: Vec<_> = due.iter().map(|(_, id, _)| *id).collect();
        assert_eq!(order, vec![t3a, t3b, t5a, t5b]);
        assert!(q.is_empty());

        // Applying in drain order into a log reproduces that order.
        let mut log = CommandLog::new();
        for (_, id, cmd) in due {
            log.push(Tick(5), id, cmd);
        }
        let logged: Vec<_> = log.entries().iter().map(|e| e.id).collect();
        assert_eq!(logged, order);
    }

    #[test]
    fn drain_leaves_future_commands() {
        let mut q = CommandQueue::new();
        q.push(TestCmd::A(1), Tick(10));
        q.push(TestCmd::A(2), Tick(4));
        let due = q.drain_due(Tick(4));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, Tick(4));
        assert_eq!(q.len(), 1);
        assert_eq!(q.drain_due(Tick(10)).len(), 1);
    }

    #[test]
    fn late_command_runs_at_next_drain() {
        let mut q = CommandQueue::new();
        q.push(TestCmd::A(1), Tick(1));
        // Tick 1 was skipped; a later drain still returns it (overdue).
        assert_eq!(q.drain_due(Tick(3)).len(), 1);
    }

    #[test]
    fn ids_are_never_reused() {
        let mut q = CommandQueue::new();
        let a = q.push(TestCmd::A(1), Tick(1));
        q.drain_due(Tick(1));
        let b = q.push(TestCmd::A(2), Tick(1));
        assert!(b > a);
    }

    #[test]
    fn serde_round_trip_keeps_order_and_next_id() {
        let mut q = CommandQueue::new();
        q.push(TestCmd::A(1), Tick(5));
        q.push(TestCmd::B { x: -2 }, Tick(3));
        let bytes = postcard::to_stdvec(&q).unwrap();
        let mut q2: CommandQueue<TestCmd> = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(q, q2);
        assert_eq!(
            q.push(TestCmd::A(9), Tick(3)),
            q2.push(TestCmd::A(9), Tick(3))
        );
        assert_eq!(q.drain_due(Tick(9)), q2.drain_due(Tick(9)));

        let mut log = CommandLog::new();
        log.push(Tick(3), CommandId(1), TestCmd::B { x: 7 });
        let bytes = postcard::to_stdvec(&log).unwrap();
        assert_eq!(
            postcard::from_bytes::<CommandLog<TestCmd>>(&bytes).unwrap(),
            log
        );
    }

    #[test]
    fn cost_rounds_once_half_away_from_zero() {
        // 12.5 per metre over 96 m is exactly 1200.00.
        assert_eq!(Cost::scaled(12.5, 96.0), Cost(120_000));
        assert_eq!(Cost::from_units(12.5), Cost(1_250));
        assert_eq!(Cost::from_units(0.0), Cost::ZERO);
        // Exactly half a cent rounds away from zero, in both directions.
        assert_eq!(Cost::scaled(1.0, 0.005), Cost(1));
        assert_eq!(Cost::scaled(-1.0, 0.005), Cost(-1));
        assert_eq!(Cost::scaled(1.0, 0.004), Cost(0));
        assert_eq!(Cost::scaled(-1.0, 0.004), Cost(0));
        assert_eq!(Cost::from_units(-12.5), Cost(-1_250));
        assert_eq!(Cost::scaled(-12.5, 96.0), Cost(-120_000));
        // One rounding per transaction: 3 × 0.333 m at 1.0/m is 99.9 → 100, not 3 × 33.
        assert_eq!(Cost::scaled(1.0, 0.999), Cost(100));
        assert_eq!(Cost(5) + Cost(7) - Cost(2), Cost(10));
        assert_eq!([Cost(1), Cost(2)].into_iter().sum::<Cost>(), Cost(3));
    }

    #[test]
    fn error_display() {
        let e = CommandError::Rejected {
            domain: "net",
            reason: "too steep".into(),
        };
        assert_eq!(e.to_string(), "net: too steep");
    }
}
