//! # fms — a Flexibilitätsmanagementsystem
//!
//! The distribution system operator's side of grid-oriented control under § 14a EnWG and
//! BNetzA BK6-22-300: determine the state of a low-voltage feeder, reduce steuerbare
//! Verbrauchseinrichtungen (SteuVE) when — and only as far and as long as — the grid needs it,
//! never below the Mindestleistung, track what is actually in effect at each Steuerbox, and keep
//! a tamper-evident log from which the documentation and the monthly VNBdigital publication are
//! computed.
//!
//! ```
//! use fms::prelude::*;
//! use jiff::civil::date;
//!
//! // What is a heat pump of 9 kW and a wallbox of 11 kW behind one EMS owed?
//! let c = Connection {
//!     id: "H1".into(), resource: None, node: "N1".into(), mode: ControlMode::Ems,
//!     area: "NB-1".into(), postcode: "34117".into(), relay_only: false, failsafe: None,
//!     failsafe_for: jiff::SignedDuration::from_hours(2), fuse: Kw(44.0),
//!     devices: vec![
//!         Device { id: "hp".into(), kind: DeviceKind::HeatPump, nominal: Kw(9.0), commissioned: date(2025, 3, 1), opted_in: false, exempt: false },
//!         Device { id: "wb".into(), kind: DeviceKind::ChargingPoint, nominal: Kw(11.0), commissioned: date(2025, 3, 1), opted_in: false, exempt: false },
//!     ],
//! };
//! let floor = RuleSet::bk6_22_300().floor(&c, date(2026, 1, 15)).unwrap();
//! assert_eq!(floor.total.round_w(), Kw(7.56)); // 4,2 + (2 − 1) · 0,8 · 4,2
//! ```
//!
//! The engine is sans-IO: [`engine::Engine::step`] takes an input and the time and returns
//! outputs. Transports, storage and the clock belong to the caller.

#![forbid(unsafe_code)]

pub mod command;
pub mod dispatch;
pub mod engine;
pub mod evidence;
pub mod grid;
pub mod model;
pub mod rules;
#[cfg(feature = "sim")]
pub mod sim;
pub mod state;
pub mod time;
pub mod units;

/// The types most programs need.
pub mod prelude {
    pub use crate::command::{Cause, Command, MessageId, Reason, Release};
    pub use crate::dispatch::State;
    pub use crate::engine::{Alarm, Engine, Input, Output};
    pub use crate::evidence::{Log, Publication, publication, publication_csv};
    pub use crate::grid::{GridSpec, Line, Topology, Transformer};
    pub use crate::model::{Connection, ConnectionId, ControlMode, Device, DeviceKind, NodeId, SrId};
    pub use crate::rules::{Regime, RuleSet};
    pub use crate::state::{Measurement, Profile, Reading};
    pub use crate::units::{Kw, PerUnit};
}
