//! What the engine sends: commands and releases, each with the reason it was sent.

use crate::model::ConnectionId;
use crate::rules::Regime;
use crate::state::{CongestionKind, Estimate};
use crate::units::Kw;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Correlates a command with its acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(pub u64);

/// A measurement a decision was computed from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRef {
    /// Meter or RTU.
    pub source: String,
    /// When it measured.
    pub at: Timestamp,
}

/// Why a command was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cause {
    /// To relieve a congestion the grid state shows.
    Relieve,
    /// To renew a reduction that is still necessary before it expires.
    Renew,
    /// On planning data, under the präventiv regime.
    Plan,
    /// A stepwise return towards release (Ziff. 4.3).
    Restore,
    /// An operator's instruction.
    Operator,
}

/// The justification every command carries. There is no command without one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    /// Why.
    pub cause: Cause,
    /// The asset whose limit is at stake.
    pub asset: String,
    /// Thermal or voltage.
    pub kind: CongestionKind,
    /// The asset's limit (kW or p.u.).
    pub limit: f64,
    /// The estimate that justified it.
    pub estimate: Estimate,
    /// The relief the asset needed, in kW (equivalent for voltage).
    pub needed: Kw,
    /// The measurements behind the estimate.
    pub inputs: Vec<InputRef>,
    /// The rule set it was computed under.
    pub rule_set: String,
    /// Who, for an operator instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Free text, for an operator instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A limit on the SteuVE consumption of one connection, for a duration.
///
/// Only the engine makes one — `#[non_exhaustive]` keeps every other crate from writing a
/// `Command { .. }` literal, and the engine builds `limit` and `floor` from a
/// [`crate::rules::Limit`], so a command never carries a value below its floor. Deserializing a
/// command reads an evidence record back; the engine never takes a `Command` as input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Command {
    /// Correlation id.
    pub msg: MessageId,
    /// The steuerbare Ressource.
    pub connection: ConnectionId,
    /// The commanded limit, never below `floor`.
    pub limit: Kw,
    /// The Mindestleistung it respects.
    pub floor: Kw,
    /// The engine wanted less and was held at the floor.
    #[serde(default)]
    pub at_floor: bool,
    /// In Direktansteuerung, the split per SteuVE.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_device: Vec<(String, Kw)>,
    /// When the limit expires by itself at the Steuerbox.
    pub valid_until: Timestamp,
    /// Under which regime.
    pub regime: Regime,
    /// Delivery attempt (1 for the first).
    pub attempt: u32,
    /// Why.
    pub reason: Reason,
}

/// The end of a reduction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Release {
    /// Correlation id.
    pub msg: MessageId,
    /// The steuerbare Ressource.
    pub connection: ConnectionId,
    /// The asset that no longer needs it.
    pub asset: String,
    /// The projected unreduced flow that made release safe.
    pub projected: Estimate,
}
