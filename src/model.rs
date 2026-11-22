//! Connections, devices, and who is a steuerbare Verbrauchseinrichtung (SteuVE).

use crate::units::Kw;
use jiff::SignedDuration;
use jiff::civil::{Date, date};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifies a Netzanschluss in the DSO's master data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectionId(pub String);

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ConnectionId {
    fn from(s: &str) -> Self {
        ConnectionId(s.to_owned())
    }
}

/// Identifies a node of the low-voltage topology.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        NodeId(s.to_owned())
    }
}

/// The ID of a steuerbare Ressource (SR-ID) in market communication: `C`, nine alphanumeric
/// characters and a check digit, per the BDEW Bildungsvorschrift.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SrId(String);

impl SrId {
    /// The check digit for the first ten characters: letters count as their ASCII code, odd
    /// positions once, even positions twice, then the distance to the next multiple of ten.
    #[must_use]
    pub fn check_digit(body: &str) -> Option<u8> {
        if body.len() != 10 || !body.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase()) {
            return None;
        }
        let value = |b: u8| if b.is_ascii_digit() { u32::from(b - b'0') } else { u32::from(b) };
        let (odd, even) = body.bytes().enumerate().fold((0, 0), |(o, e), (i, b)| if i % 2 == 0 { (o + value(b), e) } else { (o, e + value(b)) });
        let sum = odd + 2 * even;
        u8::try_from((10 - sum % 10) % 10).ok()
    }

    /// The ID as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SrId {
    type Error = String;
    fn try_from(s: String) -> Result<SrId, String> {
        let ok = s.len() == 11 && s.starts_with('C') && s.get(..10).and_then(SrId::check_digit).is_some_and(|d| s.as_bytes().get(10) == Some(&(b'0' + d)));
        if ok { Ok(SrId(s)) } else { Err(format!("{s:?} is not an SR-ID (C + 9 characters + check digit)")) }
    }
}

impl std::str::FromStr for SrId {
    type Err = String;
    fn from_str(s: &str) -> Result<SrId, String> {
        SrId::try_from(s.to_owned())
    }
}

impl From<SrId> for String {
    fn from(s: SrId) -> String {
        s.0
    }
}

impl fmt::Display for SrId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How the DSO reaches the SteuVE behind a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ControlMode {
    /// Direktansteuerung: every SteuVE is addressed and owed its own minimum.
    Direct,
    /// One value for the whole connection, distributed by the customer's energy manager.
    Ems,
}

/// The kinds of device § 14a EnWG can make controllable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKind {
    /// A non-public charging point for electric vehicles.
    ChargingPoint,
    /// A heat pump, including its auxiliary heating element.
    HeatPump,
    /// Space cooling (Anlage zur Raumkühlung).
    Cooling,
    /// A battery storage system — only its charging is controllable.
    Storage,
}

impl DeviceKind {
    /// Heat pumps and cooling are each grouped behind one connection (Ziff. 2.4.2).
    #[must_use]
    pub fn is_thermal(self) -> bool {
        matches!(self, DeviceKind::HeatPump | DeviceKind::Cooling)
    }

    /// A short human label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            DeviceKind::ChargingPoint => "wallbox",
            DeviceKind::HeatPump => "heat pump",
            DeviceKind::Cooling => "cooling",
            DeviceKind::Storage => "storage",
        }
    }
}

/// A device behind a connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// Identifier, unique within the connection.
    pub id: String,
    /// What it is.
    pub kind: DeviceKind,
    /// Netzanschlussleistung — for storage, the charging power.
    pub nominal: Kw,
    /// Commissioning date.
    pub commissioned: Date,
    /// An older device whose operator moved to the new § 14a rules.
    #[serde(default)]
    pub opted_in: bool,
    /// Exempt from participation (Ziff. 3.1.b: StVO § 35 Sonderrechte; heat pumps or cooling not
    /// serving living, office or common rooms).
    #[serde(default)]
    pub exempt: bool,
}

/// From this date new devices must participate (Ziff. 3.1.b: commissioned after 31.12.2023).
pub const SCOPE_START: Date = date(2024, 1, 1);

/// The power above which a device — or a heat-pump or cooling group — is a SteuVE.
pub const THRESHOLD: Kw = Kw(4.2);

/// One SteuVE: a single device, or the group of heat pumps (or of cooling units) behind one
/// connection, which counts as one (Ziff. 2.4.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SteuVe {
    /// The device id, or the group's ids joined by `+`.
    pub id: String,
    /// The kind.
    pub kind: DeviceKind,
    /// Its (summed) Netzanschlussleistung.
    pub nominal: Kw,
}

fn default_failsafe_for() -> SignedDuration {
    SignedDuration::from_hours(2)
}

/// A Netzanschluss behind which SteuVE may sit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    /// The connection.
    pub id: ConnectionId,
    /// Its steuerbare Ressource in market communication, if one was issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<SrId>,
    /// Where it hangs in the topology.
    pub node: NodeId,
    /// Direct or EMS.
    pub mode: ControlMode,
    /// Netzbereich (NeBe-ID) for the publication.
    pub area: String,
    /// Five-digit postcode for the publication.
    pub postcode: String,
    /// The devices behind it (controllable or not).
    pub devices: Vec<Device>,
    /// Controlled by the Steuerbox relay alone: closed holds every SteuVE at its minimum.
    #[serde(default)]
    pub relay_only: bool,
    /// The limit the Steuerbox hands the EMS for a lost channel.
    #[serde(default)]
    pub failsafe: Option<Kw>,
    /// How long the EMS holds the failsafe before it is unbound (2–24 h).
    #[serde(default = "default_failsafe_for")]
    pub failsafe_for: SignedDuration,
    /// The connection's rating; a measurement above it is implausible.
    pub fuse: Kw,
}

impl Connection {
    /// The SteuVE behind this connection on `on`.
    ///
    /// A device takes part when it is one of the four kinds, not exempt, and commissioned from
    /// 2024 or opted in. Heat pumps form one group and cooling units another; a group whose sum
    /// is above 4,2 kW is one SteuVE. Any other device above 4,2 kW is one SteuVE.
    #[must_use]
    pub fn steuve(&self, on: Date) -> Vec<SteuVe> {
        let takes_part = |d: &&Device| !d.exempt && d.commissioned <= on && (d.opted_in || d.commissioned >= SCOPE_START);
        let mut out = Vec::new();
        for kind in [DeviceKind::HeatPump, DeviceKind::Cooling] {
            let group: Vec<&Device> = self.devices.iter().filter(takes_part).filter(|d| d.kind == kind).collect();
            let nominal: Kw = group.iter().map(|d| d.nominal).sum();
            if nominal > THRESHOLD {
                out.push(SteuVe { id: group.iter().map(|d| d.id.as_str()).collect::<Vec<_>>().join("+"), kind, nominal });
            }
        }
        for d in self.devices.iter().filter(takes_part).filter(|d| !d.kind.is_thermal() && d.nominal > THRESHOLD) {
            out.push(SteuVe { id: d.id.clone(), kind: d.kind, nominal: d.nominal });
        }
        out
    }

    /// The summed Netzanschlussleistung of its SteuVE on `on`.
    #[must_use]
    pub fn steuve_nominal(&self, on: Date) -> Kw {
        self.steuve(on).iter().map(|d| d.nominal).sum()
    }
}
