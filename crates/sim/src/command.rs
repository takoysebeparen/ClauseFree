//! Commands: the only way anything outside the simulation changes it.
//!
//! Every player (or AI, console, script host) action is a [`Command`] stamped with the tick at which it takes
//! effect. The queue orders commands by `(tick, issuer, sequence)`, where `sequence` counts the commands of one
//! issuer in submission order. That order does not depend on the order in which different issuers' commands arrive,
//! which is what deterministic lockstep needs: two peers that received the same commands in a different interleaving
//! still execute them identically.

use std::collections::BTreeMap;
use std::fmt;

use data::Tag;
use data::states::StateId;

use crate::time::GameTime;

/// Who issued a command: a player slot (human or AI). Lower slots execute first within a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Issuer(pub u16);

/// What a command does.
///
/// The set grows with the systems that implement the actions. Each variant documents where its behaviour comes
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandKind {
    /// Gives control of a state to a country. Mirrors the script effect `set_state_controller_to`
    /// (`documentation/effects_documentation.md`): only the controller changes. Side effects that the original may
    /// have (on units, supply, occupation) arrive with the systems that model them.
    SetStateController { state: StateId, controller: Tag },
}

/// A command for a given tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The hour at which it executes. It must be later than the current simulation time when submitted.
    pub tick: GameTime,
    pub issuer: Issuer,
    pub kind: CommandKind,
}

/// Why a command was not accepted into the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitError {
    /// The command's tick has already been simulated (in lockstep this is a desync, not a recoverable situation).
    Late { tick: GameTime, now: GameTime },
}

impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubmitError::Late { tick, now } => write!(f, "command for {tick} arrived at {now}"),
        }
    }
}

impl std::error::Error for SubmitError {}

/// Why a queued command had no effect when it executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    UnknownState(StateId),
    UnknownCountry(Tag),
}

/// The outcome of one executed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    Rejected(Rejection),
}

/// A command as it was executed, for the command log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executed {
    pub command: Command,
    pub sequence: u64,
    pub outcome: Outcome,
}

/// Pending commands, in execution order.
#[derive(Debug, Clone, Default)]
pub struct CommandQueue {
    pending: BTreeMap<(GameTime, Issuer, u64), CommandKind>,
    next_sequence: BTreeMap<Issuer, u64>,
}

impl CommandQueue {
    /// Queues a command that executes at `command.tick`; `now` is the last simulated hour.
    pub fn submit(&mut self, command: Command, now: GameTime) -> Result<u64, SubmitError> {
        if command.tick <= now {
            return Err(SubmitError::Late { tick: command.tick, now });
        }
        let counter = self.next_sequence.entry(command.issuer).or_insert(0);
        let sequence = *counter;
        *counter += 1;
        self.pending.insert((command.tick, command.issuer, sequence), command.kind);
        Ok(sequence)
    }

    /// Removes and returns, in execution order, every command due at or before `tick`.
    pub fn take_due(&mut self, tick: GameTime) -> Vec<(Command, u64)> {
        let later = self.pending.split_off(&(tick.plus_hours(1), Issuer(0), 0));
        let due = std::mem::replace(&mut self.pending, later);
        due.into_iter().map(|((tick, issuer, sequence), kind)| (Command { tick, issuer, kind }, sequence)).collect()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(tick: i64, issuer: u16, state: u32) -> Command {
        let controller = Tag::parse("AAA").unwrap_or_else(|| panic!("tag"));
        Command {
            tick: GameTime(tick),
            issuer: Issuer(issuer),
            kind: CommandKind::SetStateController { state: StateId(state), controller },
        }
    }

    #[test]
    fn order_ignores_interleaving_between_issuers() {
        let run = |order: &[(i64, u16, u32)]| {
            let mut q = CommandQueue::default();
            for &(t, i, s) in order {
                q.submit(cmd(t, i, s), GameTime(0)).unwrap_or_else(|e| panic!("{e}"));
            }
            q.take_due(GameTime(10)).into_iter().map(|(c, _)| c).collect::<Vec<_>>()
        };
        let a = run(&[(2, 1, 10), (1, 0, 20), (2, 0, 30), (2, 1, 11)]);
        let b = run(&[(2, 0, 30), (2, 1, 10), (1, 0, 20), (2, 1, 11)]);
        assert_eq!(a, b);
        assert_eq!(a, vec![cmd(1, 0, 20), cmd(2, 0, 30), cmd(2, 1, 10), cmd(2, 1, 11)]);
    }

    #[test]
    fn late_commands_are_refused_and_future_ones_wait() {
        let mut q = CommandQueue::default();
        assert_eq!(q.submit(cmd(5, 0, 1), GameTime(5)), Err(SubmitError::Late { tick: GameTime(5), now: GameTime(5) }));
        q.submit(cmd(6, 0, 1), GameTime(5)).unwrap_or_else(|e| panic!("{e}"));
        q.submit(cmd(7, 0, 2), GameTime(5)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(q.take_due(GameTime(6)).len(), 1);
        assert_eq!(q.len(), 1);
        assert_eq!(q.take_due(GameTime(7)).len(), 1);
        assert!(q.is_empty());
    }
}
