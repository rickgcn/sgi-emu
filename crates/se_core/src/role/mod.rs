//! Bus topology roles.
//!
//! Roles describe how a component participates in a bus communication domain.
//! The core hardware model uses three roles:
//!
//! - bus: routes, arbitrates, orders, and delays transactions
//! - bus controller: initiates transactions onto a bus
//! - bus device: receives, observes, or responds to transactions on a bus
//!
//! These roles are not hardware classes. They are topological positions within
//! a communication domain. A single component may implement multiple roles.
//!
//! For example, a DMA engine can be both a bus device and a bus controller. A
//! bus bridge can be a bus device on one bus and expose another bus downstream.
//!
//! Hardware capabilities such as interrupts, reset, clocks, and DMA should be
//! modeled as protocols flowing through these roles rather than as additional
//! core role categories.

/// Role for a component that represents a bus communication domain.
///
/// A bus routes protocol transactions between bus controllers and bus devices.
/// The concrete transaction and response types are defined by the protocol
/// using the bus, not by this role.
pub trait BusRole<Transaction> {
    /// Response produced by routing the transaction.
    type Response;

    /// Routes a transaction through the bus.
    fn route(&mut self, transaction: Transaction) -> Self::Response;
}

/// Role for a component that initiates transactions onto a bus.
///
/// A bus controller receives completions for transactions it initiated. The
/// completion type is protocol-specific.
pub trait BusControllerRole<Completion> {
    /// Handles completion of a previously initiated transaction.
    fn complete(&mut self, completion: Completion);
}

/// Role for a component that receives, observes, or responds to bus traffic.
///
/// A bus device is a target or observer in a bus communication domain. The
/// concrete transaction and response types are defined by the protocol using
/// the bus device, not by this role.
pub trait BusDeviceRole<Transaction> {
    /// Response produced by accepting the transaction.
    type Response;

    /// Accepts a transaction delivered by a bus.
    fn accept(&mut self, transaction: Transaction) -> Self::Response;
}
