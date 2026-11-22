//! What is actually in effect at each steuerbare Ressource.
//!
//! ```text
//! Idle ──send──▶ Commanded ──ack──▶ Confirmed ──release/expiry──▶ Idle
//!                   │ timeout×n
//!                   ▼                               any ──channel lost──▶ Failsafe ──restored──▶ Idle
//!                Unknown ──next command──▶ Commanded
//! ```
//!
//! Planning uses [`Tracker::effective`]: the commanded value while commanded or confirmed, the
//! failsafe value in failsafe, and *nothing* when unknown — the conservative assumption for the
//! grid. Evidence keeps the unknown state, the conservative assumption for the customer.

use crate::command::{Command, MessageId};
use crate::model::ConnectionId;
use crate::units::Kw;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The believed state of one resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// No reduction in effect.
    Idle,
    /// Sent, not yet acknowledged.
    Commanded,
    /// Acknowledged by the Steuerbox.
    Confirmed,
    /// Channel lost; the Steuerbox holds its failsafe value.
    Failsafe,
    /// Not acknowledged after every attempt.
    Unknown,
}

/// A state change, recorded as evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// The resource.
    pub connection: ConnectionId,
    /// Before.
    pub from: State,
    /// After.
    pub to: State,
    /// The message concerned, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg: Option<MessageId>,
    /// What caused it.
    pub why: String,
}

#[derive(Debug, Clone)]
struct Resource {
    state: State,
    current: Option<Command>,
    deadline: Option<Timestamp>,
    failsafe: Option<Kw>,
    failsafe_for: jiff::SignedDuration,
    /// When the failsafe began; `None` once it has lapsed and the device is unbound.
    failsafe_since: Option<Timestamp>,
}

/// Parameters of delivery.
#[derive(Debug, Clone, Copy)]
pub struct Delivery {
    /// Time to wait for an acknowledgement.
    pub ack_timeout: jiff::SignedDuration,
    /// Attempts before `Unknown`.
    pub max_attempts: u32,
}

/// The per-resource state machines.
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    res: BTreeMap<ConnectionId, Resource>,
}

/// What a tick produced.
#[derive(Debug, Default)]
pub struct TickOutcome {
    /// Commands to send again.
    pub resend: Vec<Command>,
    /// State changes.
    pub transitions: Vec<Transition>,
}

impl Tracker {
    /// Register a connection and its failsafe value.
    pub fn register(&mut self, c: ConnectionId, failsafe: Option<Kw>, failsafe_for: jiff::SignedDuration) {
        self.res.entry(c).or_insert(Resource { state: State::Idle, current: None, deadline: None, failsafe, failsafe_for, failsafe_since: None });
    }

    /// The state of a resource.
    #[must_use]
    pub fn state(&self, c: &ConnectionId) -> State {
        self.res.get(c).map_or(State::Idle, |r| r.state)
    }

    /// The command in effect (or in flight), if any.
    #[must_use]
    pub fn current(&self, c: &ConnectionId) -> Option<&Command> {
        self.res.get(c).and_then(|r| r.current.as_ref())
    }

    /// The limit planning should assume.
    #[must_use]
    pub fn effective(&self, c: &ConnectionId) -> Option<Kw> {
        let r = self.res.get(c)?;
        match r.state {
            State::Commanded | State::Confirmed => r.current.as_ref().map(|c| c.limit),
            // After its duration the EMS is no longer bound by the failsafe value.
            State::Failsafe => r.failsafe.filter(|_| r.failsafe_since.is_some()),
            State::Idle | State::Unknown => None,
        }
    }

    /// Record that a command is being sent.
    pub fn send(&mut self, cmd: &Command, now: Timestamp, d: Delivery) -> Option<Transition> {
        let r = self.res.get_mut(&cmd.connection)?;
        r.current = Some(cmd.clone());
        r.deadline = Some(now.saturating_add(d.ack_timeout).unwrap_or(now));
        if r.state == State::Failsafe {
            // The channel is down: the Steuerbox rejects whatever arrives during the fault (FNN 2a).
            // The command is recorded as held, not silently dropped.
            return Some(Transition {
                connection: cmd.connection.clone(),
                from: State::Failsafe,
                to: State::Failsafe,
                msg: Some(cmd.msg),
                why: "command held: the channel is down, the Steuerbox keeps its failsafe".into(),
            });
        }
        let from = r.state;
        r.state = State::Commanded;
        (from != State::Commanded).then(|| Transition {
            connection: cmd.connection.clone(),
            from,
            to: State::Commanded,
            msg: Some(cmd.msg),
            why: "sent".into(),
        })
    }

    /// Record that a reduction ended (released by the engine).
    pub fn release(&mut self, c: &ConnectionId, msg: MessageId) -> Option<Transition> {
        let r = self.res.get_mut(c)?;
        r.current = None;
        r.deadline = None;
        if r.state == State::Failsafe {
            return None;
        }
        let from = std::mem::replace(&mut r.state, State::Idle);
        (from != State::Idle).then(|| Transition { connection: c.clone(), from, to: State::Idle, msg: Some(msg), why: "released".into() })
    }

    /// An acknowledgement arrived. `Err` names why it was ignored.
    ///
    /// # Errors
    /// When the message is unknown or superseded — it is never applied.
    pub fn ack(&mut self, c: &ConnectionId, msg: MessageId) -> Result<Option<Transition>, String> {
        let r = self.res.get_mut(c).ok_or_else(|| "unknown resource".to_owned())?;
        match &r.current {
            Some(cur) if cur.msg == msg => {}
            Some(_) => return Err("superseded message".into()),
            None => return Err("no command in flight".into()),
        }
        r.deadline = None;
        if r.state == State::Confirmed || r.state == State::Failsafe {
            return Ok(None);
        }
        let from = std::mem::replace(&mut r.state, State::Confirmed);
        Ok(Some(Transition { connection: c.clone(), from, to: State::Confirmed, msg: Some(msg), why: "acknowledged".into() }))
    }

    /// The channel to the Steuerbox is lost: it falls back to its failsafe value.
    pub fn channel_lost(&mut self, c: &ConnectionId, now: Timestamp) -> Option<Transition> {
        let r = self.res.get_mut(c)?;
        if r.state != State::Failsafe {
            r.failsafe_since = Some(now);
        }
        let from = std::mem::replace(&mut r.state, State::Failsafe);
        (from != State::Failsafe).then(|| Transition { connection: c.clone(), from, to: State::Failsafe, msg: None, why: "channel lost".into() })
    }

    /// The channel is back: the failsafe ends; whatever is still needed is commanded afresh.
    pub fn channel_restored(&mut self, c: &ConnectionId) -> Option<Transition> {
        let r = self.res.get_mut(c)?;
        if r.state != State::Failsafe {
            return None;
        }
        r.state = State::Idle;
        r.current = None;
        r.deadline = None;
        r.failsafe_since = None;
        Some(Transition { connection: c.clone(), from: State::Failsafe, to: State::Idle, msg: None, why: "channel restored".into() })
    }

    /// Advance time: retries, timeouts, expiry.
    pub fn tick(&mut self, now: Timestamp, d: Delivery) -> TickOutcome {
        let mut out = TickOutcome::default();
        for (id, r) in &mut self.res {
            if r.state == State::Failsafe
                && let Some(since) = r.failsafe_since
                && since.saturating_add(r.failsafe_for).unwrap_or(since) <= now
            {
                r.failsafe_since = None;
                out.transitions.push(Transition {
                    connection: id.clone(),
                    from: State::Failsafe,
                    to: State::Failsafe,
                    msg: None,
                    why: "failsafe duration over; the EMS is unbound".into(),
                });
            }
            let Some(cur) = r.current.as_mut() else { continue };
            if cur.valid_until <= now {
                let from = r.state;
                r.current = None;
                r.deadline = None;
                if matches!(from, State::Commanded | State::Confirmed | State::Unknown) {
                    r.state = State::Idle;
                    out.transitions.push(Transition { connection: id.clone(), from, to: State::Idle, msg: None, why: "expired".into() });
                }
                continue;
            }
            if r.state == State::Commanded && r.deadline.is_some_and(|dl| dl <= now) {
                if cur.attempt < d.max_attempts {
                    cur.attempt += 1;
                    r.deadline = Some(now.saturating_add(d.ack_timeout * i32::try_from(cur.attempt).unwrap_or(1)).unwrap_or(now));
                    out.resend.push(cur.clone());
                } else {
                    r.state = State::Unknown;
                    r.deadline = None;
                    out.transitions.push(Transition {
                        connection: id.clone(),
                        from: State::Commanded,
                        to: State::Unknown,
                        msg: Some(cur.msg),
                        why: format!("no acknowledgement after {} attempts", cur.attempt),
                    });
                }
            }
        }
        out
    }
}
