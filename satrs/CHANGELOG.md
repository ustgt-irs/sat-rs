Change Log
=======

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/)
and this project adheres to [Semantic Versioning](http://semver.org/).

# [unreleased]

- Added `hk` module helpers to track whether a single HK set needs regeneration:
  `SingleSetHkHelperStd` (`std`), `SingleSetHkHelperEmbassy` (new `embassy-time` feature),
  and `SingleSetHkHelperCountdown`, generic over the existing `Countdown` trait.
- Added `fdir::RecoveryFdir` (`std`), which escalates component faults to a power cycle
  recovery first and to a faulty component if it has to be recovered too often.
- Added `fdir::RecoveryEvent` for recovery related events.

# [v0.3.0-alpha.3] 2025-11-06

- Bump `sat-rs` edition to 2024.
- Bumped `spacepackets` to v0.17
- `ComponentId` is u32 now
- Simplified TCP servers

## Changed

Some trait renaming to be more in-line with Rust naming conventions.

- `EventTmHookProvider` -> `EventTmHook`
- `ActiveRequestProvider` -> `ActiveRequest`
- `EcssTcInMemConversionProvider` -> `CacheAndReadRawEcssTc`
- `ActiveRequestMapProvider` -> `ActiveRequestStore`
- `CountdownProvider` -> `Countdown`

# [v0.3.0-alpha.2] 2025-07-22

`satrs-shared` update

# [v0.3.0-alpha.1] 2025-07-22

`spacepackets` range v0.14 to v0.15

# [v0.3.0-alpha.0] 2025-02-18

`spacepackets` v0.13

## Changed

- Renamed `StaticPoolConfig::new` to `StaticPoolConfig::new_from_subpool_cfg_tuples`. The new
  `new` implementation expects a type struct instead of tuples.
- Moved `cfdp` module to [dedicated crate](https://egit.irs.uni-stuttgart.de/rust/cfdp)
- Moved `seq_count` module to [spacepackets](https://egit.irs.uni-stuttgart.de/rust/spacepackets)
  crate

## Added

- `StaticHeaplessMemoryPool` which can be grown with user-provided static buffers.
- Scheduling table for systems with a standard runtime
- Mode Tree Feature which allows building a network of mode components which can send mode
  messages to each other.
- Added first helper features like the `SubsystemExecutionHelper` and the
  `SubsystemCommandingHelper` which allows to build subsystem components. Subsystem components
  are able to execute mode sequences and perform target keeping based on a declarative table
  format.
- Added `DevManagerCommandingHelper` which performs some of the boilerplate logik required
  by Assembly and Device Management components. This includes forwarding mode requests and
  handling mode replies.
- First basic health module with `HealthState`s and the `HealthTableProvider` trait. These
  components are important for any FDIR components which get added in the future.

# [v0.2.1] 2024-05-19

## Changed

- The HAL TCP server `ServerConfig::new` method now sets the `reuse_port` and `reuse_addr`
  fields to `true`.

## Fixed

- Possibly subtly broken v0.2.0 build artifact.

# [v0.2.0] 2024-05-02

## Changed

- Various improvements for the PUS stack components.

## Added

- Added `HandlingStatus` enumeration.

# [v0.2.0-rc.5] 2024-04-24

## Added

- Optional `defmt::Format` support for the event types, if the `defmt` feature is activated.

## Changed

- Removed `MpscEventReceiver`, the `EventReceiveProvider` trait is implemented directly
  on `mpsc::Receiver<EventMessage<Event>>`
- Renamed `PusEventDispatcher` to `PusEventTmCreatorWithMap`.
- Renamed `DefaultPusEventU32Dispatcher` to `DefaultPusEventU32EventCreator`.
- Renamed `PusEventMgmtBackendProvider` renamed to `PusEventReportingMap`.
- Reanmed Event `const_new` methods to `new` and the former `new` methods to `new_checked`

# [v0.2.0-rc.4] 2024-04-23

## Changed

- The `parse_for_ccsds_space_packets` method now expects a non-mutable slice and does not copy
  broken tail packets anymore. It also does not expect a mutable `next_write_idx` argument anymore.
  Instead, a `ParseResult` structure is returned which contains the `packets_found` and an
  optional `incomplete_tail_start` value.

## Fixed

- `parse_for_ccsds_space_packets` did not detect CCSDS space packets at the buffer end with the
  smallest possible size of 7 bytes.
- TCP server component now re-registers the internal `mio::Poll` object if the client reset
  the connection unexpectedly. Not doing so prevented the server from functioning properly
  after a re-connect.

# [v0.2.0-rc.3] 2024-04-17

docs-rs hotfix 2

# [v0.2.0-rc.2] 2024-04-17

docs-rs hotfix

# [v0.2.0-rc.1] 2024-04-17

- `spacepackets` v0.11

## Added

- Added `params::WritableToBeBytes::to_vec`.
- New `ComponentId` (`u64` typedef for now) which replaces former `TargetId` as a generic
  way to identify components.
- Various abstraction and objects for targeted requests. This includes mode request/reply
  types for actions, HK and modes.
- `VerificationReportingProvider::owner_id` method.
- Introduced generic `EventMessage` which is generic over the event type and the additional
  parameter type. This message also contains the sender ID which can be useful for debugging
  or application layer / FDIR logic.
- Stop signal handling for the TCP servers.
- TCP server now uses `mio` crate to allow non-blocking operation. The server can now handle
  multiple connections at once, and the context information about handled transfers is
  passed via a callback which is inserted as a generic as well.

## Changed

- Renamed `ReceivesTcCore` to `PacketSenderRaw` to better show its primary purpose. It now contains
  a `send_raw_tc` method which is not mutable anymore.
- Renamed `TmPacketSourceCore` to `TmPacketSource`.
- Renamed `EcssTmSenderCore` to `EcssTmSender`.
- Renamed `StoreAddr` to `PoolAddr`.
- Reanmed `StoreError` to `PoolError`.
- TCP server generics order. The error generics come last now.
- `encoding::ccsds::PacketIdValidator` renamed to `ValidatorU16Id`, which lives in the crate root.
  It can be used for both CCSDS packet ID and CCSDS APID validation.
- `EventManager::try_event_handling` not expects a mutable error handling closure instead of
  returning the occured errors.
- Renamed `EventManagerBase` to `EventReportCreator`
- Renamed `VerificationReporterCore` to `VerificationReportCreator`.
- Removed `VerificationReporterCore`. The high-level API exposed by `VerificationReporter` and
  the low level API exposed by `VerificationReportCreator` should be sufficient for all use-cases.
- Refactored `EventManager` to heavily use generics instead of trait objects.
  - `SendEventProvider` -> `EventSendProvider`. `id` trait method renamed to `channel_id`.
  - `ListenerTable` -> `ListenerMapProvider`
  - `SenderTable` -> `SenderMapProvider`
  - There is an `EventManagerWithMpsc` and a `EventManagerWithBoundedMpsc` helper type now.
- Refactored ECSS TM sender abstractions to be generic over different message queue backends.
- Refactored Verification Reporter abstractions and implementation to be generic over the sender
  instead of using trait objects.
- Renamed `WritableToBeBytes::raw_len` to `WritableToBeBytes::written_len` for consistency.
- `PusServiceProvider` renamed to `PusServiceDistributor` to make the purpose of the object
  more clear
- `PusServiceProvider::handle_pus_tc_packet` renamed to `PusServiceDistributor::distribute_packet`.
- `PusServiceDistibutor` and `CcsdsDistributor` now use generics instead of trait objects.
  This makes accessing the concrete trait implementations more easy as well.
- Major overhaul of the PUS handling module.
- Replace `TargetId` by `ComponentId`.
- Replace most usages of `ChannelId` by `ComponentId`. A dedicated channel ID has limited usage
  due to the nature of typed channels in Rust.
- `CheckTimer` renamed to `CountdownProvider`.
- Renamed `TargetId` to `ComponentId`.
- Replaced most `ChannelId` occurences with `ComponentId`. For typed channels, there is generally
  no need for dedicated channel IDs.
- Changed `params::WritableToBeBytes::raw_len` to `written_len` for consistency.
- `EventReporter` caches component ID.
- Renamed `PusService11SchedHandler` to `PusSchedServiceHandler`.
- Fixed general naming of PUS handlers from `handle_one_tc` to `poll_and_handle_next_tc`.
- Reworked verification module: The sender (`impl EcssTmSenderCore`)
  now needs to be passed explicitely to the `VerificationReportingProvider` abstraction. This
  allows easier sharing of the TM sender component.

## Fixed

- Update deprecated API for `PusScheduler::insert_wrapped_tc_cds_short`
  and `PusScheduler::insert_wrapped_tc_cds_long`.
- `EventReporter` uses interior mutability pattern to allow non-mutable API.

## Removed

- Remove `objects` module.
- Removed CCSDS and PUS distributor modules. Their worth is questionable in an architecture
  where routing traits are sufficient and the core logic to demultiplex and distribute packets
  is simple enough to be application code.

# [v0.2.0-rc.0] 2024-02-21

## Added

- New PUS service abstractions for HK (PUS 3) and actions (PUS 8). Introducing new abstractions
  allows to move some boilerplate code into the framework.
- New `VerificationReportingProvider` abstraction to avoid relying on a concrete verification
  reporting provider.

## Changed

- Verification reporter API timestamp arguments are not `Option`al anymore. Empty timestamps
  can be passed by simply specifying the `&[]` empty slice argument.

# [v0.1.1] 2024-02-12

- Minor fixes for crate config `homepage` entries and links in documentation.

# [v0.1.0] 2024-02-12

Initial release.

[unreleased]: https://egit.irs.uni-stuttgart.de/rust/sat-rs/compare/satrs-v0.3.0-alpha.3...HEAD
[v0.3.0-alpha.3]: https://egit.irs.uni-stuttgart.de/rust/sat-rs/compare/satrs-v0.3.0-alpha.2...satrs-v0.3.0-alpha.3
[v0.3.0-alpha.2]: https://egit.irs.uni-stuttgart.de/rust/sat-rs/compare/satrs-v0.3.0-alpha.1...satrs-v0.3.0-alpha.2
[v0.3.0-alpha.1]: https://egit.irs.uni-stuttgart.de/rust/sat-rs/compare/satrs-v0.3.0-alpha.0...satrs-v0.3.0-alpha.1
[v0.3.0-alpha.0]: https://egit.irs.uni-stuttgart.de/rust/sat-rs/compare/satrs-v0.2.1...satrs-v0.3.0-alpha.0
