//! Engine protocol between the engine service and its clients: envelope, bootstrap frames and
//! session messages (ADR-018, D-038).
//!
//! frame = magic `TLR1` | proto u16 | flags u8 | encoded_len u32 | decoded_len u32 | payload
//!
//! All integers are little endian. Session payloads are postcard. `Hello`, `Welcome` and
//! `Refused` are bootstrap frames: never compressed, and their payload starts with a fixed prefix
//! that never changes between versions (proto u16, app_version as a u16 byte length plus UTF-8,
//! flags u32), so a peer on another proto can always read a refusal. tl-remote adds zstd (flag
//! bit 0); this module checks the header for it and never decompresses.
//!
//! Every length is checked against `MAX_FRAME_LEN` before a payload buffer is allocated. Blocking
//! `std::io` only. Errors carry sizes only, never payload bytes.

use std::io::{ErrorKind, Read};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::actions::ActionEvent;
use crate::command::Command;
use crate::demand::Demand;
use crate::journal::{JournalControl, JournalEvent, JournalQuery};
use crate::time::BootId;

/// First 4 bytes of every frame.
pub const MAGIC: [u8; 4] = *b"TLR1";
/// Protocol version. Peers are compatible only when their `proto` values are equal.
pub const PROTO: u16 = 1;
/// The app version sent in bootstrap frames.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Magic, proto, flags, encoded length, decoded length.
pub const HEADER_LEN: usize = 15;
/// Largest encoded or decoded payload of one frame: 64 MiB.
pub const MAX_FRAME_LEN: usize = 64 * 1024 * 1024;
/// Envelope flag bit 0: the payload is zstd (tl-remote).
pub const FLAG_ZSTD: u8 = 0x01;
/// Longest `app_version` in a bootstrap prefix, in bytes.
pub const APP_VERSION_MAX: usize = 64;
/// Bootstrap prefix flag bit 0: this server frame is `Refused`, not `Welcome`. Other bits are
/// ignored on read, so a later version may use them.
pub const BOOT_REFUSED: u32 = 0x01;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("frame does not start with TLR1")]
    BadMagic,
    #[error("unknown envelope flags {flags:#04x}")]
    UnknownFlags { flags: u8 },
    #[error("frame of {len} bytes exceeds the limit of {max}")]
    TooLarge { len: usize, max: usize },
    /// An uncompressed frame whose two lengths differ.
    #[error("encoded length {encoded} differs from decoded length {decoded}")]
    LengthMismatch { encoded: u32, decoded: u32 },
    /// EOF inside a header or payload, or a payload shorter than its content.
    #[error("truncated frame")]
    Truncated,
    /// Clean EOF before the first byte of a frame.
    #[error("end of stream")]
    Eof,
    #[error("{extra} bytes after the end of the message")]
    TrailingBytes { extra: usize },
    /// The payload must be decompressed first (tl-remote), or a bootstrap frame is compressed.
    #[error("compressed payload")]
    Compressed,
    #[error("peer speaks protocol {theirs}, this side speaks {ours}")]
    ProtoMismatch { ours: u16, theirs: u16 },
    /// The envelope's proto and the bootstrap prefix's proto differ, or the frame kind is wrong.
    #[error("malformed bootstrap frame")]
    BadBootstrap,
    #[error("text is not UTF-8")]
    BadUtf8,
    #[error("text of {len} bytes exceeds the limit of {max}")]
    TextTooLong { len: usize, max: usize },
    #[error("decode error: {0}")]
    Decode(postcard::Error),
    #[error("encode error: {0}")]
    Encode(postcard::Error),
    #[error("i/o error: {0}")]
    Io(std::io::Error),
}

// ---- Envelope ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrameHeader {
    pub proto: u16,
    pub flags: u8,
    pub encoded_len: u32,
    pub decoded_len: u32,
}

impl FrameHeader {
    /// An uncompressed frame of `len` payload bytes.
    pub fn plain(proto: u16, len: u32) -> Self {
        Self {
            proto,
            flags: 0,
            encoded_len: len,
            decoded_len: len,
        }
    }

    pub fn is_compressed(&self) -> bool {
        self.flags & FLAG_ZSTD != 0
    }

    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4..6].copy_from_slice(&self.proto.to_le_bytes());
        b[6] = self.flags;
        b[7..11].copy_from_slice(&self.encoded_len.to_le_bytes());
        b[11..15].copy_from_slice(&self.decoded_len.to_le_bytes());
        b
    }

    /// Checks the magic, the flags and both lengths. The proto is not checked here: bootstrap
    /// frames from another proto must still be readable.
    pub fn parse(b: &[u8; HEADER_LEN]) -> Result<Self, ProtoError> {
        if b[0..4] != MAGIC {
            return Err(ProtoError::BadMagic);
        }
        let header = Self {
            proto: u16::from_le_bytes([b[4], b[5]]),
            flags: b[6],
            encoded_len: u32::from_le_bytes([b[7], b[8], b[9], b[10]]),
            decoded_len: u32::from_le_bytes([b[11], b[12], b[13], b[14]]),
        };
        if header.flags & !FLAG_ZSTD != 0 {
            return Err(ProtoError::UnknownFlags {
                flags: header.flags,
            });
        }
        for len in [header.encoded_len, header.decoded_len] {
            if len as usize > MAX_FRAME_LEN {
                return Err(ProtoError::TooLarge {
                    len: len as usize,
                    max: MAX_FRAME_LEN,
                });
            }
        }
        if !header.is_compressed() && header.encoded_len != header.decoded_len {
            return Err(ProtoError::LengthMismatch {
                encoded: header.encoded_len,
                decoded: header.decoded_len,
            });
        }
        Ok(header)
    }
}

/// One frame as read: the checked header and `encoded_len` payload bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RawFrame {
    pub header: FrameHeader,
    pub payload: Vec<u8>,
}

/// Reads one frame. The header is checked before the payload buffer is allocated, so an
/// oversize length leaves the reader just past the header.
pub fn read_frame<R: Read>(r: &mut R) -> Result<RawFrame, ProtoError> {
    let mut b = [0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        match r.read(&mut b[filled..]) {
            Ok(0) if filled == 0 => return Err(ProtoError::Eof),
            Ok(0) => return Err(ProtoError::Truncated),
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(ProtoError::Io(e)),
        }
    }
    let header = FrameHeader::parse(&b)?;
    let mut payload = vec![0u8; header.encoded_len as usize];
    r.read_exact(&mut payload).map_err(|e| match e.kind() {
        ErrorKind::UnexpectedEof => ProtoError::Truncated,
        _ => ProtoError::Io(e),
    })?;
    Ok(RawFrame { header, payload })
}

/// Appends one uncompressed session frame holding `msg` to `out`. On any error `out` is left as
/// it was.
pub fn encode_message<T: Serialize>(msg: &T, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    let len = postcard::serialize_with_flavor(msg, postcard::ser_flavors::Size::default())
        .map_err(ProtoError::Encode)?;
    if len > MAX_FRAME_LEN {
        return Err(ProtoError::TooLarge {
            len,
            max: MAX_FRAME_LEN,
        });
    }
    let start = out.len();
    out.reserve(HEADER_LEN + len);
    // len <= MAX_FRAME_LEN < u32::MAX, checked above.
    out.extend_from_slice(&FrameHeader::plain(PROTO, len as u32).to_bytes());
    if let Err(e) = postcard::to_extend(msg, VecExtend(out)) {
        out.truncate(start);
        return Err(ProtoError::Encode(e));
    }
    Ok(())
}

/// Decodes an uncompressed session frame of this proto.
pub fn decode_message<T: DeserializeOwned>(frame: &RawFrame) -> Result<T, ProtoError> {
    if frame.header.is_compressed() {
        return Err(ProtoError::Compressed);
    }
    if frame.header.proto != PROTO {
        return Err(ProtoError::ProtoMismatch {
            ours: PROTO,
            theirs: frame.header.proto,
        });
    }
    decode_payload(&frame.payload)
}

/// Decodes one postcard value that must fill `payload` exactly. tl-remote calls this on the
/// decompressed payload.
pub fn decode_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ProtoError> {
    let (msg, rest) = postcard::take_from_bytes(payload).map_err(|e| match e {
        postcard::Error::DeserializeUnexpectedEnd => ProtoError::Truncated,
        e => ProtoError::Decode(e),
    })?;
    if !rest.is_empty() {
        return Err(ProtoError::TrailingBytes { extra: rest.len() });
    }
    Ok(msg)
}

/// Lets postcard append to a borrowed `Vec` so `encode_message` can restore it on error.
struct VecExtend<'a>(&'a mut Vec<u8>);

impl Extend<u8> for VecExtend<'_> {
    fn extend<I: IntoIterator<Item = u8>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}

// ---- Bootstrap frames ----

/// The fixed start of every bootstrap payload. Its layout never changes between versions.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BootPrefix {
    pub proto: u16,
    /// At most `APP_VERSION_MAX` bytes.
    pub app_version: Arc<str>,
    pub flags: u32,
}

impl BootPrefix {
    /// This build's proto and app version, no flags.
    pub fn ours() -> Self {
        Self {
            proto: PROTO,
            app_version: Arc::from(APP_VERSION),
            flags: 0,
        }
    }

    pub fn is_compatible(&self) -> bool {
        self.proto == PROTO
    }
}

/// What a client asks of its session.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Want {
    /// False for a control-only client such as `tasklens-setup` changing host settings.
    pub snapshots: bool,
}

/// The `Hello` tail, read only when the proto matches.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HelloTail {
    /// The window's process id, so the KWin bridge can tell tasklens windows apart.
    pub pid: u32,
    pub app_id: Arc<str>,
    /// The connection comes through `tasklens bridge`, so the session is remote.
    pub bridge: bool,
    pub want: Want,
}

/// The first frame a client sends.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hello {
    pub prefix: BootPrefix,
    /// None when decoded from another proto.
    pub tail: Option<HelloTail>,
}

/// The `Welcome` tail: who the engine is (ADR-018 Identity and reconnects).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HostIdentity {
    pub host_name: Arc<str>,
    /// /etc/machine-id. The client keys a host by it.
    pub machine_id: Arc<str>,
    pub boot_id: BootId,
    /// 128 random bits per engine process.
    pub engine_instance: u128,
}

/// The server's answer when it accepts a `Hello`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Welcome {
    pub prefix: BootPrefix,
    /// None when decoded from another proto.
    pub tail: Option<HostIdentity>,
}

/// Why the server refused a session. The code is part of the fixed layout.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RefuseReason {
    /// The peers' `proto` values differ.
    Proto,
    /// The server already has its most sessions.
    TooManySessions,
    /// The `Hello` could not be read.
    BadHello,
    /// A code this version does not know.
    Other(u16),
}

impl RefuseReason {
    pub fn code(self) -> u16 {
        match self {
            RefuseReason::Proto => 1,
            RefuseReason::TooManySessions => 2,
            RefuseReason::BadHello => 3,
            RefuseReason::Other(code) => code,
        }
    }

    pub fn from_code(code: u16) -> Self {
        match code {
            1 => RefuseReason::Proto,
            2 => RefuseReason::TooManySessions,
            3 => RefuseReason::BadHello,
            other => RefuseReason::Other(other),
        }
    }
}

/// The server's answer when it refuses a `Hello`. It names the server's proto and app version in
/// its prefix and has no tail.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Refused {
    pub prefix: BootPrefix,
    pub reason: RefuseReason,
    /// At most `u16::MAX` bytes.
    pub text: Arc<str>,
}

impl Refused {
    /// The refusal for a client on another proto, with this build's prefix.
    pub fn proto_mismatch(client: &BootPrefix) -> Self {
        Self {
            prefix: BootPrefix::ours(),
            reason: RefuseReason::Proto,
            text: Arc::from(format!(
                "the engine runs tasklens {APP_VERSION} (protocol {PROTO}), the window runs \
                 tasklens {} (protocol {})",
                client.app_version, client.proto
            )),
        }
    }
}

/// The first frame a client reads.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ServerBootstrap {
    Welcome(Welcome),
    Refused(Refused),
}

/// Appends a `Hello` frame. The envelope proto is `hello.prefix.proto`.
pub fn encode_hello(hello: &Hello, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    let mut body = Vec::new();
    put_prefix(&hello.prefix, hello.prefix.flags & !BOOT_REFUSED, &mut body)?;
    if let Some(tail) = &hello.tail {
        postcard::to_extend(tail, VecExtend(&mut body)).map_err(ProtoError::Encode)?;
    }
    push_bootstrap(hello.prefix.proto, &body, out)
}

/// Appends a `Welcome` frame.
pub fn encode_welcome(welcome: &Welcome, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    let mut body = Vec::new();
    put_prefix(
        &welcome.prefix,
        welcome.prefix.flags & !BOOT_REFUSED,
        &mut body,
    )?;
    if let Some(tail) = &welcome.tail {
        postcard::to_extend(tail, VecExtend(&mut body)).map_err(ProtoError::Encode)?;
    }
    push_bootstrap(welcome.prefix.proto, &body, out)
}

/// Appends a `Refused` frame: prefix with `BOOT_REFUSED` set, reason code u16, reason text as a
/// u16 byte length plus UTF-8.
pub fn encode_refused(refused: &Refused, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    let mut body = Vec::new();
    put_prefix(
        &refused.prefix,
        refused.prefix.flags | BOOT_REFUSED,
        &mut body,
    )?;
    body.extend_from_slice(&refused.reason.code().to_le_bytes());
    put_str16(&refused.text, u16::MAX as usize, &mut body)?;
    push_bootstrap(refused.prefix.proto, &body, out)
}

/// Reads a client's first frame. The tail is decoded only when the proto matches; from another
/// proto the result has the prefix and `tail: None`, so the server can answer with `Refused`.
pub fn decode_hello(frame: &RawFrame) -> Result<Hello, ProtoError> {
    let (prefix, rest) = bootstrap_prefix(frame)?;
    if prefix.flags & BOOT_REFUSED != 0 {
        return Err(ProtoError::BadBootstrap);
    }
    let tail = if prefix.is_compatible() {
        Some(decode_payload(rest)?)
    } else {
        None
    };
    Ok(Hello { prefix, tail })
}

/// Reads a server's first frame: `Refused` when the prefix has `BOOT_REFUSED`, else `Welcome`.
/// A `Refused` is readable from any proto.
pub fn decode_server_bootstrap(frame: &RawFrame) -> Result<ServerBootstrap, ProtoError> {
    let (prefix, rest) = bootstrap_prefix(frame)?;
    if prefix.flags & BOOT_REFUSED != 0 {
        let mut cur = Cursor(rest);
        let reason = RefuseReason::from_code(cur.u16()?);
        let text = cur.str16(u16::MAX as usize)?;
        cur.finish()?;
        return Ok(ServerBootstrap::Refused(Refused {
            prefix,
            reason,
            text,
        }));
    }
    let tail = if prefix.is_compatible() {
        Some(decode_payload(rest)?)
    } else {
        None
    };
    Ok(ServerBootstrap::Welcome(Welcome { prefix, tail }))
}

fn put_prefix(prefix: &BootPrefix, flags: u32, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    out.extend_from_slice(&prefix.proto.to_le_bytes());
    put_str16(&prefix.app_version, APP_VERSION_MAX, out)?;
    out.extend_from_slice(&flags.to_le_bytes());
    Ok(())
}

fn put_str16(s: &str, max: usize, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    if s.len() > max {
        return Err(ProtoError::TextTooLong { len: s.len(), max });
    }
    // max <= u16::MAX, so the length fits.
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

fn push_bootstrap(proto: u16, body: &[u8], out: &mut Vec<u8>) -> Result<(), ProtoError> {
    if body.len() > MAX_FRAME_LEN {
        return Err(ProtoError::TooLarge {
            len: body.len(),
            max: MAX_FRAME_LEN,
        });
    }
    out.extend_from_slice(&FrameHeader::plain(proto, body.len() as u32).to_bytes());
    out.extend_from_slice(body);
    Ok(())
}

/// Checks the envelope of a bootstrap frame and reads its prefix. Returns the bytes after it.
fn bootstrap_prefix(frame: &RawFrame) -> Result<(BootPrefix, &[u8]), ProtoError> {
    if frame.header.is_compressed() {
        return Err(ProtoError::Compressed);
    }
    let mut cur = Cursor(&frame.payload);
    let proto = cur.u16()?;
    let app_version = cur.str16(APP_VERSION_MAX)?;
    let flags = cur.u32()?;
    if proto != frame.header.proto {
        return Err(ProtoError::BadBootstrap);
    }
    Ok((
        BootPrefix {
            proto,
            app_version,
            flags,
        },
        cur.0,
    ))
}

/// Reads the fixed little-endian fields of the bootstrap layout.
struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        if self.0.len() < n {
            return Err(ProtoError::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u16(&mut self) -> Result<u16, ProtoError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, ProtoError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn str16(&mut self, max: usize) -> Result<Arc<str>, ProtoError> {
        let len = self.u16()? as usize;
        if len > max {
            return Err(ProtoError::TextTooLong { len, max });
        }
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(Arc::from)
            .map_err(|_| ProtoError::BadUtf8)
    }

    fn finish(self) -> Result<(), ProtoError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ProtoError::TrailingBytes {
                extra: self.0.len(),
            })
        }
    }
}

// ---- Session messages ----

/// One journal stream of a session. Allocated by the client, starting at 1.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StreamId(pub u32);

/// Body of one snapshot frame (ADR-018 Snapshot deltas). tasklens-q4x76.3 adds the parts.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct SnapshotFrame {
    /// Numbered from 1 per session.
    pub frame: u64,
    /// The frame this is a delta against; 0 means complete.
    pub base: u64,
}

/// Values of the host settings in `engine.toml`. tasklens-q4x76.9 adds the fields.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct HostSettingsValues {}

/// Changes to host settings, sent with `SetHostSettings`. tasklens-q4x76.9 adds the fields.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct HostSettingsChanges {}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct HostSettings {
    pub rev: u64,
    pub values: HostSettingsValues,
}

/// The reply to `SetHostSettings`. On `Conflict` the client reloads.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SetHostSettingsReply {
    Ack { rev: u64 },
    Conflict { current: HostSettings },
}

/// Client to server, after `Welcome`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    Command(Command),
    /// The session's own demand.
    SetDemand(Demand),
    /// The client has applied and installed this snapshot frame.
    Ack {
        frame: u64,
    },
    JournalOpen {
        stream: StreamId,
        query: JournalQuery,
    },
    JournalCtl {
        stream: StreamId,
        control: JournalControl,
    },
    /// Sent from the journal handle's `Drop`.
    JournalClose {
        stream: StreamId,
    },
    GetHostSettings,
    SetHostSettings {
        base_rev: u64,
        changes: HostSettingsChanges,
    },
}

impl ClientMsg {
    /// `Ack` and `JournalClose` go on the control path, ahead of the command queue, so they work
    /// when that queue is full (ADR-018 Queues in bytes).
    pub fn is_control(&self) -> bool {
        matches!(self, ClientMsg::Ack { .. } | ClientMsg::JournalClose { .. })
    }
}

/// Server to client, after `Welcome`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    Snapshot(SnapshotFrame),
    Action(ActionEvent),
    Journal {
        stream: StreamId,
        event: JournalEvent,
    },
    /// The reply to `GetHostSettings`.
    HostSettings(HostSettings),
    SetHostSettings(SetHostSettingsReply),
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use proptest::prelude::*;
    use smallvec::smallvec;

    use super::*;
    use crate::actions::{
        ActionError, ActionId, ActionKind, ActionRequest, ActionState, ActionTarget, CgroupRef,
        Family, FourTuple, LaunchSpec, LaunchWhat, Proto, RecoveryEdit, RejectReason, Signal,
    };
    use crate::command::{DeviceCommand, HistoryWhat, KWinCommand, SetupStep};
    use crate::demand::{CpuDemand, DetailDemand, DeviceHistoryRequest, DeviceRange};
    use crate::details::{DetailTab, DetailTarget, ScanId, SearchId, StringScanOpts};
    use crate::health::SourceId;
    use crate::history::{HistoryMetric, HistoryRange};
    use crate::ids::{ColumnId, GraphTarget, GroupKey, NodeId, ProcessKey};
    use crate::journal::{JournalAccess, JournalEntry, JournalError};
    use crate::snapshot::FrameGeometry;
    use crate::system::CpuSet;
    use crate::table::{
        Manager, ServiceFilter, StartupRef, StartupSource, SysColumnId, SysNodeId, SysSortSpec,
        SysTable, SysTableQuery, TableQuery, UnitRef,
    };

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: 1000 + pid as u64,
            boot_id: BootId(42),
            pid,
        }
    }

    fn unit(name: &str) -> UnitRef {
        UnitRef {
            manager: Manager::User,
            name: Arc::from(name),
        }
    }

    fn one_frame(bytes: &[u8]) -> RawFrame {
        let mut r = bytes;
        let frame = read_frame(&mut r).unwrap();
        assert!(r.is_empty(), "one frame expected");
        frame
    }

    /// Encodes, reads back, decodes and encodes again. Both encodings and the Debug text match.
    fn round_trip<T: Serialize + DeserializeOwned + std::fmt::Debug>(msg: &T) {
        let mut bytes = Vec::new();
        encode_message(msg, &mut bytes).unwrap();
        let back: T = decode_message(&one_frame(&bytes)).unwrap();
        let mut again = Vec::new();
        encode_message(&back, &mut again).unwrap();
        assert_eq!(bytes, again);
        assert_eq!(format!("{msg:?}"), format!("{back:?}"));
    }

    fn all_commands() -> Vec<Command> {
        let keys: Arc<[ProcessKey]> = Arc::from([key(7), key(8)]);
        let socket = ActionTarget::Socket {
            cookie: 99,
            family: Family::Inet6,
            proto: Proto::Tcp,
            four_tuple: FourTuple {
                local: "[::1]:631".parse::<SocketAddr>().unwrap(),
                remote: "10.0.0.2:443".parse::<SocketAddr>().unwrap(),
            },
        };
        let mut cpus = CpuSet::default();
        cpus.insert(3);
        vec![
            Command::SetTableQuery(TableQuery {
                generation: 4,
                filter: Arc::from("fire"),
                expanded: Arc::from([
                    NodeId::Group(GroupKey(5)),
                    NodeId::Process(key(1)),
                    NodeId::KernelThreads,
                    NodeId::Unit(unit("a.service")),
                ]),
                mem_group: Some(Arc::from("exe:claude")),
                ..TableQuery::default()
            }),
            Command::SetSysTableQuery(SysTableQuery {
                table: SysTable::Services,
                generation: 2,
                sort: SysSortSpec {
                    column: SysColumnId::all(SysTable::Services)[3],
                    ascending: false,
                },
                filter: Arc::from("ssh"),
                expanded: Arc::from([
                    SysNodeId::ServiceGroup(Manager::System),
                    SysNodeId::UnitProcess(unit("b.service"), key(2)),
                    SysNodeId::Startup(StartupRef {
                        source: StartupSource::AutostartUser,
                        id: Arc::from("x.desktop"),
                    }),
                    SysNodeId::Conn(17),
                ]),
                services: Some(ServiceFilter::default()),
            }),
            Command::Action(ActionRequest {
                id: ActionId(1),
                kind: ActionKind::Signal(Signal::Other(40)),
                targets: smallvec![
                    ActionTarget::Process(key(3)),
                    ActionTarget::Thread {
                        process: key(3),
                        tid: 4,
                    },
                    ActionTarget::Cgroup(CgroupRef {
                        rel_path: Arc::from("user.slice"),
                        cgroup_id: 12,
                    }),
                    ActionTarget::Unit(unit("c.service")),
                    socket,
                ],
                privileged: true,
            }),
            Command::Action(ActionRequest {
                id: ActionId(2),
                kind: ActionKind::SetAffinity(cpus),
                targets: smallvec![ActionTarget::Process(key(5))],
                privileged: false,
            }),
            Command::Action(ActionRequest {
                id: ActionId(3),
                kind: ActionKind::UnitWriteRecovery(RecoveryEdit {
                    restart: Some(Arc::from("on-failure")),
                    on_failure: Some(Arc::from([Arc::from("notify.service")])),
                    ..RecoveryEdit::default()
                }),
                targets: smallvec![ActionTarget::Unit(unit("d.service"))],
                privileged: false,
            }),
            Command::Action(ActionRequest {
                id: ActionId(4),
                kind: ActionKind::Launch(LaunchSpec {
                    what: LaunchWhat::DbusActivate {
                        bus_name: Arc::from("org.kde.dolphin"),
                        object_path: Arc::from("/org/kde/dolphin"),
                        action: Arc::from("open"),
                        parameter: None,
                    },
                    as_admin: false,
                    cwd: Some(Arc::from("/home")),
                }),
                targets: smallvec![],
                privileged: false,
            }),
            Command::RefreshNow(SourceId::Cpu),
            Command::ResetBaselines,
            Command::ReprobeSource(SourceId::Sensors),
            Command::KWin(KWinCommand::SetFrameGeometry {
                pid: 10,
                caption: Arc::from("CPU"),
                geometry: FrameGeometry {
                    x: -5,
                    y: 6,
                    width: 300,
                    height: 200,
                },
            }),
            Command::KWin(KWinCommand::KeepAbove {
                pid: 10,
                caption: None,
                on: true,
            }),
            Command::DetailRefresh {
                target: DetailTarget::App(GroupKey(9)),
                tab: DetailTab::AppProcesses,
            },
            Command::ComputeHashes(key(6)),
            Command::VerifyPackage(key(6)),
            Command::StringScan {
                scan: ScanId(1),
                key: key(6),
                opts: StringScanOpts::default(),
            },
            Command::CancelStringScan(ScanId(1)),
            Command::HandleSearch {
                search: SearchId(2),
                pattern: Arc::from("libc"),
                include_maps: true,
                privileged: false,
            },
            Command::CancelHandleSearch(SearchId(2)),
            Command::HistoryQuery {
                id: 3,
                range: HistoryRange::Span { from: 1, to: 2 },
                what: HistoryWhat::Frame {
                    metric: HistoryMetric::Gpu,
                    view: (1, 2),
                    keys: keys.clone(),
                    overview: true,
                },
            },
            Command::HistoryDelete { keys },
            Command::SetupStatus,
            Command::RunSetup {
                step: SetupStep::PolkitRule { on: true },
            },
            Command::Device(DeviceCommand::Eject {
                seq: 1,
                disk: Arc::from("sda"),
            }),
            Command::StartHelperSession,
            Command::EndHelperSession,
        ]
    }

    fn all_action_states() -> Vec<ActionState> {
        vec![
            ActionState::Rejected(RejectReason::NotOwner),
            ActionState::Started,
            ActionState::Succeeded,
            ActionState::Failed(ActionError::UnitJob {
                result: Arc::from("failed"),
                error_name: Some(Arc::from("org.freedesktop.systemd1.NoSuchUnit")),
            }),
            ActionState::Failed(ActionError::Partial { done: 1, total: 3 }),
            ActionState::OutcomeUnknown(Arc::from("connection to home lost")),
        ]
    }

    /// No wildcard arm: a new `ClientMsg` variant breaks this test until it is covered.
    fn client_name(msg: &ClientMsg) -> &'static str {
        match msg {
            ClientMsg::Command(_) => "command",
            ClientMsg::SetDemand(_) => "set_demand",
            ClientMsg::Ack { .. } => "ack",
            ClientMsg::JournalOpen { .. } => "journal_open",
            ClientMsg::JournalCtl { .. } => "journal_ctl",
            ClientMsg::JournalClose { .. } => "journal_close",
            ClientMsg::GetHostSettings => "get_host_settings",
            ClientMsg::SetHostSettings { .. } => "set_host_settings",
        }
    }

    /// No wildcard arm: a new `ServerMsg` variant breaks this test until it is covered.
    fn server_name(msg: &ServerMsg) -> &'static str {
        match msg {
            ServerMsg::Snapshot(_) => "snapshot",
            ServerMsg::Action(_) => "action",
            ServerMsg::Journal { .. } => "journal",
            ServerMsg::HostSettings(_) => "host_settings",
            ServerMsg::SetHostSettings(_) => "set_host_settings",
        }
    }

    #[test]
    fn client_messages_round_trip() {
        let mut msgs: Vec<ClientMsg> = all_commands().into_iter().map(ClientMsg::Command).collect();
        let demand = Demand {
            any_surface_visible: true,
            cpu: CpuDemand::PerCore,
            detail_keys: Arc::from([key(1)]),
            details: Arc::from([DetailDemand {
                target: DetailTarget::Process(key(1)),
                tab: DetailTab::Maps,
            }]),
            unit_details: Arc::from([unit("e.service")]),
            graph_targets: Arc::from([
                (GraphTarget::Name(Arc::from("firefox")), ColumnId::Cpu),
                (GraphTarget::Process(key(2)), ColumnId::Name),
            ]),
            device_history_range: Some(DeviceHistoryRequest {
                range: DeviceRange::H24,
                end_realtime_ns: Some(-1),
            }),
            ..Demand::default()
        };
        let mut process_columns = demand.process_columns;
        process_columns.insert(ColumnId::Cpu);
        msgs.extend([
            ClientMsg::SetDemand(Demand {
                process_columns,
                ..demand
            }),
            ClientMsg::Ack { frame: u64::MAX },
            ClientMsg::JournalOpen {
                stream: StreamId(1),
                query: JournalQuery {
                    unit: unit("f.service"),
                    follow: true,
                },
            },
            ClientMsg::JournalCtl {
                stream: StreamId(1),
                control: JournalControl::LoadOlder(Some(Arc::from("s=abc;i=1"))),
            },
            ClientMsg::JournalCtl {
                stream: StreamId(1),
                control: JournalControl::SetFollow(false),
            },
            ClientMsg::JournalClose {
                stream: StreamId(1),
            },
            ClientMsg::GetHostSettings,
            ClientMsg::SetHostSettings {
                base_rev: 7,
                changes: HostSettingsChanges::default(),
            },
        ]);
        let mut names: Vec<_> = msgs.iter().map(client_name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 8);
        for msg in &msgs {
            round_trip(msg);
        }
        assert!(ClientMsg::Ack { frame: 1 }.is_control());
        assert!(
            ClientMsg::JournalClose {
                stream: StreamId(1)
            }
            .is_control()
        );
        assert!(!ClientMsg::GetHostSettings.is_control());
    }

    #[test]
    fn server_messages_round_trip() {
        let mut msgs = vec![ServerMsg::Snapshot(SnapshotFrame { frame: 2, base: 1 })];
        msgs.extend(all_action_states().into_iter().map(|state| {
            ServerMsg::Action(ActionEvent {
                id: ActionId(9),
                target: Some(2),
                state,
            })
        }));
        let entry = JournalEntry {
            realtime_us: 1_700_000_000_000_000,
            priority: 6,
            pid: Some(42),
            message: Arc::from("started"),
            cursor: Arc::from("s=abc;i=1"),
        };
        msgs.extend([
            ServerMsg::Journal {
                stream: StreamId(3),
                event: JournalEvent::Batch {
                    entries: Arc::from([entry.clone(), entry]),
                    older: true,
                    exhausted: false,
                },
            },
            ServerMsg::Journal {
                stream: StreamId(3),
                event: JournalEvent::Access(JournalAccess::OwnOnly),
            },
            ServerMsg::Journal {
                stream: StreamId(3),
                event: JournalEvent::Error(JournalError::Exit {
                    code: 1,
                    stderr_first_line: Arc::from("no such unit"),
                }),
            },
            ServerMsg::HostSettings(HostSettings {
                rev: 5,
                values: HostSettingsValues::default(),
            }),
            ServerMsg::SetHostSettings(SetHostSettingsReply::Ack { rev: 6 }),
            ServerMsg::SetHostSettings(SetHostSettingsReply::Conflict {
                current: HostSettings::default(),
            }),
        ]);
        let mut names: Vec<_> = msgs.iter().map(server_name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 5);
        for msg in &msgs {
            round_trip(msg);
        }
    }

    fn hello() -> Hello {
        Hello {
            prefix: BootPrefix::ours(),
            tail: Some(HelloTail {
                pid: 4242,
                app_id: Arc::from("io.github.oysteinkrog.Tasklens"),
                bridge: true,
                want: Want { snapshots: true },
            }),
        }
    }

    fn identity() -> HostIdentity {
        HostIdentity {
            host_name: Arc::from("home"),
            machine_id: Arc::from("0123456789abcdef0123456789abcdef"),
            boot_id: BootId(77),
            engine_instance: u128::MAX - 3,
        }
    }

    #[test]
    fn bootstrap_round_trip() {
        let mut bytes = Vec::new();
        encode_hello(&hello(), &mut bytes).unwrap();
        assert_eq!(decode_hello(&one_frame(&bytes)).unwrap(), hello());

        let welcome = Welcome {
            prefix: BootPrefix::ours(),
            tail: Some(identity()),
        };
        bytes.clear();
        encode_welcome(&welcome, &mut bytes).unwrap();
        assert_eq!(
            decode_server_bootstrap(&one_frame(&bytes)).unwrap(),
            ServerBootstrap::Welcome(welcome)
        );

        for reason in [
            RefuseReason::Proto,
            RefuseReason::TooManySessions,
            RefuseReason::BadHello,
            RefuseReason::Other(900),
        ] {
            let refused = Refused {
                prefix: BootPrefix::ours(),
                reason,
                text: Arc::from("nein"),
            };
            bytes.clear();
            encode_refused(&refused, &mut bytes).unwrap();
            let ServerBootstrap::Refused(back) =
                decode_server_bootstrap(&one_frame(&bytes)).unwrap()
            else {
                panic!("expected Refused");
            };
            assert_eq!(back.reason, reason);
            assert_eq!(back.prefix.flags, BOOT_REFUSED);
            assert_eq!(&*back.text, "nein");
        }
    }

    /// The bootstrap layout never changes once shipped. This pins every byte of a `Refused`.
    #[test]
    fn refused_layout_is_fixed() {
        let refused = Refused {
            prefix: BootPrefix {
                proto: 0x0102,
                app_version: Arc::from("1.2"),
                flags: 0,
            },
            reason: RefuseReason::TooManySessions,
            text: Arc::from("full"),
        };
        let mut bytes = Vec::new();
        encode_refused(&refused, &mut bytes).unwrap();
        #[rustfmt::skip]
        let expected: &[u8] = &[
            b'T', b'L', b'R', b'1',
            0x02, 0x01,             // proto
            0x00,                   // envelope flags
            19, 0, 0, 0,            // encoded length
            19, 0, 0, 0,            // decoded length
            0x02, 0x01,             // prefix proto
            3, 0, b'1', b'.', b'2', // app_version
            1, 0, 0, 0,             // flags: BOOT_REFUSED
            2, 0,                   // reason code
            4, 0, b'f', b'u', b'l', b'l',
        ];
        assert_eq!(bytes, expected);
    }

    /// A `Hello` from another proto, with a tail this version cannot read, still gives a readable
    /// prefix; the `Refused` built from it is readable by that other version.
    #[test]
    fn hello_from_other_proto_gets_readable_refused() {
        let mut future = Vec::new();
        let other = BootPrefix {
            proto: PROTO + 1,
            app_version: Arc::from("9.0.0"),
            flags: 0x8000_0000,
        };
        put_prefix(&other, other.flags, &mut future).unwrap();
        future.extend_from_slice(&[0xff; 40]);
        let mut bytes = Vec::new();
        push_bootstrap(other.proto, &future, &mut bytes).unwrap();

        let got = decode_hello(&one_frame(&bytes)).unwrap();
        assert_eq!(got.prefix, other);
        assert_eq!(got.tail, None);
        assert!(!got.prefix.is_compatible());

        let refused = Refused::proto_mismatch(&got.prefix);
        bytes.clear();
        encode_refused(&refused, &mut bytes).unwrap();
        let frame = one_frame(&bytes);
        // The other side reads only the fixed layout, whatever its proto.
        let ServerBootstrap::Refused(back) = decode_server_bootstrap(&frame).unwrap() else {
            panic!("expected Refused");
        };
        assert_eq!(back.reason, RefuseReason::Proto);
        assert_eq!(back.prefix.proto, PROTO);
        assert_eq!(&*back.prefix.app_version, APP_VERSION);
        assert!(back.text.contains("9.0.0"));
        assert!(back.text.contains(APP_VERSION));
    }

    #[test]
    fn welcome_from_other_proto_has_no_tail() {
        let mut body = Vec::new();
        let other = BootPrefix {
            proto: PROTO + 1,
            app_version: Arc::from("9.0.0"),
            flags: 0,
        };
        put_prefix(&other, 0, &mut body).unwrap();
        body.push(0xff);
        let mut bytes = Vec::new();
        push_bootstrap(other.proto, &body, &mut bytes).unwrap();
        assert_eq!(
            decode_server_bootstrap(&one_frame(&bytes)).unwrap(),
            ServerBootstrap::Welcome(Welcome {
                prefix: other,
                tail: None
            })
        );
    }

    fn header_bytes(proto: u16, flags: u8, encoded: u32, decoded: u32) -> Vec<u8> {
        FrameHeader {
            proto,
            flags,
            encoded_len: encoded,
            decoded_len: decoded,
        }
        .to_bytes()
        .to_vec()
    }

    #[test]
    fn malformed_headers_are_errors() {
        let mut bad_magic = header_bytes(PROTO, 0, 0, 0);
        bad_magic[3] = b'2';
        assert!(matches!(
            read_frame(&mut &bad_magic[..]),
            Err(ProtoError::BadMagic)
        ));
        assert!(matches!(
            read_frame(&mut &header_bytes(PROTO, 0x02, 0, 0)[..]),
            Err(ProtoError::UnknownFlags { flags: 0x02 })
        ));
        assert!(matches!(
            read_frame(&mut &header_bytes(PROTO, 0, 4, 5)[..]),
            Err(ProtoError::LengthMismatch {
                encoded: 4,
                decoded: 5
            })
        ));
        // A compressed frame may have different lengths; tl-remote decodes it.
        let mut zstd = header_bytes(PROTO, FLAG_ZSTD, 2, 500);
        zstd.extend_from_slice(&[1, 2]);
        let frame = one_frame(&zstd);
        assert!(frame.header.is_compressed());
        assert!(matches!(
            decode_message::<ClientMsg>(&frame),
            Err(ProtoError::Compressed)
        ));
        assert!(matches!(decode_hello(&frame), Err(ProtoError::Compressed)));
    }

    #[test]
    fn oversize_frames_are_errors_before_allocation() {
        let over = MAX_FRAME_LEN as u32 + 1;
        // Only the header is present: a reader that allocated first would report Truncated.
        assert!(matches!(
            read_frame(&mut &header_bytes(PROTO, 0, over, over)[..]),
            Err(ProtoError::TooLarge { len, max: MAX_FRAME_LEN }) if len == over as usize
        ));
        assert!(matches!(
            read_frame(&mut &header_bytes(PROTO, FLAG_ZSTD, 10, over)[..]),
            Err(ProtoError::TooLarge { .. })
        ));
        assert!(matches!(
            read_frame(&mut &header_bytes(PROTO, FLAG_ZSTD, u32::MAX, 10)[..]),
            Err(ProtoError::TooLarge { .. })
        ));
        // The limit itself is allowed.
        let max = MAX_FRAME_LEN as u32;
        assert!(FrameHeader::parse(&FrameHeader::plain(PROTO, max).to_bytes()).is_ok());

        let huge = ServerMsg::Journal {
            stream: StreamId(1),
            event: JournalEvent::Error(JournalError::Exit {
                code: 1,
                stderr_first_line: Arc::from("x".repeat(MAX_FRAME_LEN)),
            }),
        };
        let mut out = vec![7u8];
        assert!(matches!(
            encode_message(&huge, &mut out),
            Err(ProtoError::TooLarge { .. })
        ));
        assert_eq!(out, [7]);
    }

    #[test]
    fn truncated_frames_are_errors() {
        assert!(matches!(read_frame(&mut &[][..]), Err(ProtoError::Eof)));
        let mut bytes = Vec::new();
        encode_message(&ClientMsg::Ack { frame: 300 }, &mut bytes).unwrap();
        for cut in 1..bytes.len() {
            assert!(
                matches!(read_frame(&mut &bytes[..cut]), Err(ProtoError::Truncated)),
                "cut at {cut}"
            );
        }
        // The envelope is whole but the payload ends inside the message.
        let mut short = header_bytes(PROTO, 0, 1, 1);
        short.push(2); // ClientMsg::Ack, frame missing
        assert!(matches!(
            decode_message::<ClientMsg>(&one_frame(&short)),
            Err(ProtoError::Truncated)
        ));
        // A bootstrap prefix that ends early.
        bytes.clear();
        encode_hello(&hello(), &mut bytes).unwrap();
        let frame = one_frame(&bytes);
        for cut in 0..8 {
            let short = RawFrame {
                header: FrameHeader::plain(PROTO, cut as u32),
                payload: frame.payload[..cut].to_vec(),
            };
            assert!(
                matches!(decode_hello(&short), Err(ProtoError::Truncated)),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn trailing_bytes_are_errors() {
        let mut bytes = Vec::new();
        encode_message(&ClientMsg::GetHostSettings, &mut bytes).unwrap();
        let mut frame = one_frame(&bytes);
        frame.payload.push(0);
        frame.header = FrameHeader::plain(PROTO, frame.payload.len() as u32);
        assert!(matches!(
            decode_message::<ClientMsg>(&frame),
            Err(ProtoError::TrailingBytes { extra: 1 })
        ));

        bytes.clear();
        encode_hello(&hello(), &mut bytes).unwrap();
        let mut frame = one_frame(&bytes);
        frame.payload.push(0);
        assert!(matches!(
            decode_hello(&frame),
            Err(ProtoError::TrailingBytes { extra: 1 })
        ));

        bytes.clear();
        encode_refused(&Refused::proto_mismatch(&BootPrefix::ours()), &mut bytes).unwrap();
        let mut frame = one_frame(&bytes);
        frame.payload.extend_from_slice(&[1, 2]);
        assert!(matches!(
            decode_server_bootstrap(&frame),
            Err(ProtoError::TrailingBytes { extra: 2 })
        ));
    }

    #[test]
    fn malformed_bootstrap_is_error() {
        // Envelope proto and prefix proto differ.
        let mut body = Vec::new();
        put_prefix(&BootPrefix::ours(), 0, &mut body).unwrap();
        let mut bytes = Vec::new();
        push_bootstrap(PROTO + 1, &body, &mut bytes).unwrap();
        assert!(matches!(
            decode_hello(&one_frame(&bytes)),
            Err(ProtoError::BadBootstrap)
        ));
        // A Hello with the refused bit.
        body.clear();
        put_prefix(&BootPrefix::ours(), BOOT_REFUSED, &mut body).unwrap();
        bytes.clear();
        push_bootstrap(PROTO, &body, &mut bytes).unwrap();
        assert!(matches!(
            decode_hello(&one_frame(&bytes)),
            Err(ProtoError::BadBootstrap)
        ));
        // app_version over 64 bytes, on write and on read.
        let long = BootPrefix {
            app_version: Arc::from("v".repeat(APP_VERSION_MAX + 1)),
            ..BootPrefix::ours()
        };
        let long_hello = Hello {
            prefix: long,
            tail: None,
        };
        assert!(matches!(
            encode_hello(&long_hello, &mut Vec::new()),
            Err(ProtoError::TextTooLong { len: 65, max: 64 })
        ));
        let mut raw = PROTO.to_le_bytes().to_vec();
        raw.extend_from_slice(&65u16.to_le_bytes());
        raw.extend_from_slice(&[b'v'; 65]);
        raw.extend_from_slice(&0u32.to_le_bytes());
        let frame = RawFrame {
            header: FrameHeader::plain(PROTO, raw.len() as u32),
            payload: raw,
        };
        assert!(matches!(
            decode_hello(&frame),
            Err(ProtoError::TextTooLong { len: 65, max: 64 })
        ));
        // app_version that is not UTF-8.
        let mut raw = PROTO.to_le_bytes().to_vec();
        raw.extend_from_slice(&2u16.to_le_bytes());
        raw.extend_from_slice(&[0xc3, 0x28]);
        raw.extend_from_slice(&0u32.to_le_bytes());
        let frame = RawFrame {
            header: FrameHeader::plain(PROTO, raw.len() as u32),
            payload: raw,
        };
        assert!(matches!(decode_hello(&frame), Err(ProtoError::BadUtf8)));
        // A Hello tail that is not a HelloTail.
        body.clear();
        put_prefix(&BootPrefix::ours(), 0, &mut body).unwrap();
        body.push(0x80);
        bytes.clear();
        push_bootstrap(PROTO, &body, &mut bytes).unwrap();
        assert!(decode_hello(&one_frame(&bytes)).is_err());
    }

    #[test]
    fn message_from_other_proto_is_error() {
        let mut bytes = Vec::new();
        encode_message(&ClientMsg::GetHostSettings, &mut bytes).unwrap();
        let mut frame = one_frame(&bytes);
        frame.header.proto = PROTO + 1;
        assert!(matches!(
            decode_message::<ClientMsg>(&frame),
            Err(ProtoError::ProtoMismatch { ours: PROTO, theirs }) if theirs == PROTO + 1
        ));
    }

    #[test]
    fn app_version_fits_prefix() {
        assert!(APP_VERSION.len() <= APP_VERSION_MAX);
    }

    proptest! {
        /// Arbitrary bytes never panic any decoder, and a frame read from them never holds more
        /// than its header allows.
        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let mut r = &bytes[..];
            if let Ok(frame) = read_frame(&mut r) {
                prop_assert_eq!(frame.payload.len(), frame.header.encoded_len as usize);
                let _ = decode_message::<ClientMsg>(&frame);
                let _ = decode_message::<ServerMsg>(&frame);
                let _ = decode_hello(&frame);
                let _ = decode_server_bootstrap(&frame);
            }
        }

        /// Arbitrary payloads behind a valid header never panic the decoders.
        #[test]
        fn arbitrary_payload_never_panics(payload in proptest::collection::vec(any::<u8>(), 0..256)) {
            let frame = RawFrame {
                header: FrameHeader::plain(PROTO, payload.len() as u32),
                payload,
            };
            let _ = decode_message::<ClientMsg>(&frame);
            let _ = decode_message::<ServerMsg>(&frame);
            let _ = decode_hello(&frame);
            let _ = decode_server_bootstrap(&frame);
        }
    }
}
