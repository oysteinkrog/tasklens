//! The envelope's zstd layer (ADR-018 Envelope, Envelope details).
//!
//! Session payloads larger than `COMPRESS_ABOVE` bytes are zstd level 1 with flag bit 0 set.
//! Bootstrap frames are never compressed; read them with `FrameReader::read_frame` and decode
//! them with tl-model's `decode_hello` and `decode_server_bootstrap`, which refuse a compressed
//! frame.
//!
//! Memory limits on the reading side, all checked before the buffer they guard is allocated:
//! - each length at most `MAX_FRAME_LEN` (64 MiB), checked by `FrameHeader::parse`;
//! - encoded plus decoded bytes of the one frame being read at most `SESSION_INPUT_MAX` (80 MiB).
//!   One snapshot is in flight per session, so no other input is buffered;
//! - decompression into one buffer of exactly the declared decoded length, which cannot grow;
//! - a zstd window of at most 8 MiB (`windowLogMax 23`).

use std::fmt;
use std::io::{ErrorKind, Read};

use tl_model::remote_proto::{
    ClientMsg, FLAG_ZSTD, FrameHeader, HEADER_LEN, MAX_FRAME_LEN, PROTO, ProtoError, RawFrame,
    ServerMsg, decode_payload, encode_message,
};
use zstd::zstd_safe::{
    self, CCtx, CParameter, DCtx, DParameter, InBuffer, OutBuffer, ResetDirective,
};

/// Encoded plus decoded bytes of the one frame a session is reading: 80 MiB.
pub const SESSION_INPUT_MAX: usize = 80 * 1024 * 1024;
/// Payloads longer than this many bytes are compressed.
pub const COMPRESS_ABOVE: usize = 1024;
/// zstd level for every compressed payload.
pub const ZSTD_LEVEL: i32 = 1;
/// The decoder refuses a zstd window larger than 2^23 bytes (8 MiB).
pub const WINDOW_LOG_MAX: u32 = 23;
const WINDOW_MAX: u64 = 1 << WINDOW_LOG_MAX;
/// Magic number at the start of a standard zstd frame (RFC 8878).
const ZSTD_MAGIC: u32 = 0xFD2F_B528;

#[derive(Debug)]
pub enum EnvelopeError {
    Proto(ProtoError),
    /// Encoded plus decoded bytes of one frame exceed `SESSION_INPUT_MAX`.
    InputCap {
        len: usize,
        max: usize,
    },
    /// The payload ends before its declared decoded length.
    DecodedShort {
        declared: usize,
        got: usize,
    },
    /// The payload decompresses past its declared decoded length.
    DecodedLong {
        declared: usize,
    },
    /// The zstd frame needs a window larger than 8 MiB.
    WindowTooLarge {
        size: u64,
        max: u64,
    },
    /// A zstd error, by zstd's own name for it.
    Zstd(&'static str),
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnvelopeError::Proto(e) => e.fmt(f),
            EnvelopeError::InputCap { len, max } => {
                write!(f, "frame needs {len} bytes of input, the limit is {max}")
            }
            EnvelopeError::DecodedShort { declared, got } => {
                write!(
                    f,
                    "payload decompresses to {got} bytes, {declared} declared"
                )
            }
            EnvelopeError::DecodedLong { declared } => {
                write!(f, "payload decompresses past its declared {declared} bytes")
            }
            EnvelopeError::WindowTooLarge { size, max } => {
                write!(f, "zstd window of {size} bytes exceeds the limit of {max}")
            }
            EnvelopeError::Zstd(name) => write!(f, "zstd error: {name}"),
        }
    }
}

impl std::error::Error for EnvelopeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EnvelopeError::Proto(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ProtoError> for EnvelopeError {
    fn from(e: ProtoError) -> Self {
        EnvelopeError::Proto(e)
    }
}

fn zstd_error(code: usize) -> EnvelopeError {
    EnvelopeError::Zstd(zstd_safe::get_error_name(code))
}

/// Encodes session messages into frames, compressing payloads above `COMPRESS_ABOVE` bytes.
/// Keeps its buffers and zstd context between frames.
pub struct FrameWriter {
    scratch: Vec<u8>,
    packed: Vec<u8>,
    cctx: CCtx<'static>,
}

impl FrameWriter {
    pub fn new() -> Result<Self, EnvelopeError> {
        let mut cctx = CCtx::create();
        cctx.set_parameter(CParameter::CompressionLevel(ZSTD_LEVEL))
            .map_err(zstd_error)?;
        Ok(Self {
            scratch: Vec::new(),
            packed: Vec::new(),
            cctx,
        })
    }

    /// Appends one frame holding `msg` to `out`. On any error `out` is left as it was.
    pub fn encode_client(
        &mut self,
        msg: &ClientMsg,
        out: &mut Vec<u8>,
    ) -> Result<(), EnvelopeError> {
        self.scratch.clear();
        encode_message(msg, &mut self.scratch)?;
        self.finish(out)
    }

    /// Appends one frame holding `msg` to `out`. On any error `out` is left as it was.
    /// `Proto(TooLarge)` means the frame would break a limit the reader enforces.
    pub fn encode_server(
        &mut self,
        msg: &ServerMsg,
        out: &mut Vec<u8>,
    ) -> Result<(), EnvelopeError> {
        self.scratch.clear();
        encode_message(msg, &mut self.scratch)?;
        self.finish(out)
    }

    /// Moves the plain frame in `scratch` to `out`, compressed when its payload is large.
    fn finish(&mut self, out: &mut Vec<u8>) -> Result<(), EnvelopeError> {
        let payload = &self.scratch[HEADER_LEN..];
        if payload.len() <= COMPRESS_ABOVE {
            out.extend_from_slice(&self.scratch);
            return Ok(());
        }
        self.packed.clear();
        self.packed
            .reserve(zstd_safe::compress_bound(payload.len()));
        self.cctx
            .compress2(&mut self.packed, payload)
            .map_err(zstd_error)?;
        let encoded = self.packed.len();
        // The reader refuses these, so the sender never sends them.
        if encoded > MAX_FRAME_LEN {
            return Err(ProtoError::TooLarge {
                len: encoded,
                max: MAX_FRAME_LEN,
            }
            .into());
        }
        if encoded + payload.len() > SESSION_INPUT_MAX {
            return Err(ProtoError::TooLarge {
                len: encoded + payload.len(),
                max: SESSION_INPUT_MAX,
            }
            .into());
        }
        let header = FrameHeader {
            proto: PROTO,
            flags: FLAG_ZSTD,
            // Both at most MAX_FRAME_LEN < u32::MAX: encode_message and the check above.
            encoded_len: encoded as u32,
            decoded_len: payload.len() as u32,
        };
        out.reserve(HEADER_LEN + encoded);
        out.extend_from_slice(&header.to_bytes());
        out.extend_from_slice(&self.packed);
        Ok(())
    }
}

/// Reads frames and decodes session messages, decompressing within the limits above. Keeps one
/// zstd context, reset for each frame.
pub struct FrameReader {
    dctx: DCtx<'static>,
}

impl FrameReader {
    pub fn new() -> Result<Self, EnvelopeError> {
        let mut dctx = DCtx::create();
        dctx.set_parameter(DParameter::WindowLogMax(WINDOW_LOG_MAX))
            .map_err(zstd_error)?;
        Ok(Self { dctx })
    }

    /// Reads one frame and leaves its payload as sent. Both lengths and the session input cap
    /// are checked before the payload buffer is allocated, so a refused frame leaves the reader
    /// just past its header.
    pub fn read_frame<R: Read>(&self, r: &mut R) -> Result<RawFrame, EnvelopeError> {
        let mut b = [0u8; HEADER_LEN];
        let mut filled = 0;
        while filled < HEADER_LEN {
            match r.read(&mut b[filled..]) {
                Ok(0) if filled == 0 => return Err(ProtoError::Eof.into()),
                Ok(0) => return Err(ProtoError::Truncated.into()),
                Ok(n) => filled += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(ProtoError::Io(e).into()),
            }
        }
        let header = FrameHeader::parse(&b)?;
        let encoded = header.encoded_len as usize;
        let input = if header.is_compressed() {
            encoded + header.decoded_len as usize
        } else {
            encoded
        };
        if input > SESSION_INPUT_MAX {
            return Err(EnvelopeError::InputCap {
                len: input,
                max: SESSION_INPUT_MAX,
            });
        }
        let mut payload = vec![0u8; encoded];
        r.read_exact(&mut payload).map_err(|e| match e.kind() {
            ErrorKind::UnexpectedEof => ProtoError::Truncated,
            _ => ProtoError::Io(e),
        })?;
        Ok(RawFrame { header, payload })
    }

    pub fn decode_client(&mut self, frame: &RawFrame) -> Result<ClientMsg, EnvelopeError> {
        let mut buf = Vec::new();
        Ok(decode_payload(self.payload(frame, &mut buf)?)?)
    }

    pub fn decode_server(&mut self, frame: &RawFrame) -> Result<ServerMsg, EnvelopeError> {
        let mut buf = Vec::new();
        Ok(decode_payload(self.payload(frame, &mut buf)?)?)
    }

    /// The session payload of `frame`, decompressed into `buf` when the frame is compressed.
    fn payload<'a>(
        &mut self,
        frame: &'a RawFrame,
        buf: &'a mut Vec<u8>,
    ) -> Result<&'a [u8], EnvelopeError> {
        if frame.header.proto != PROTO {
            return Err(ProtoError::ProtoMismatch {
                ours: PROTO,
                theirs: frame.header.proto,
            }
            .into());
        }
        if !frame.header.is_compressed() {
            return Ok(&frame.payload);
        }
        // Checked against MAX_FRAME_LEN and the session cap when the frame was read.
        *buf = vec![0u8; frame.header.decoded_len as usize];
        self.decompress(&frame.payload, buf)?;
        Ok(buf)
    }

    /// Decompresses one zstd frame that must fill `dst` exactly. `dst` is a slice, so nothing
    /// here can grow it past the declared length.
    fn decompress(&mut self, src: &[u8], dst: &mut [u8]) -> Result<(), EnvelopeError> {
        // zstd takes a single-pass path that skips the windowLogMax check when the whole frame
        // and its content fit, so the window is checked here as well.
        if let Some(size) = frame_window_size(src)
            && size > WINDOW_MAX
        {
            return Err(EnvelopeError::WindowTooLarge {
                size,
                max: WINDOW_MAX,
            });
        }
        self.dctx
            .reset(ResetDirective::SessionOnly)
            .map_err(zstd_error)?;
        let declared = dst.len();
        let mut input = InBuffer::around(src);
        let mut output = OutBuffer::around(dst);
        loop {
            let before = (input.pos(), output.pos());
            let hint = self
                .dctx
                .decompress_stream(&mut output, &mut input)
                .map_err(zstd_error)?;
            if hint == 0 {
                // The frame is complete and flushed.
                if input.pos() < src.len() {
                    return Err(ProtoError::TrailingBytes {
                        extra: src.len() - input.pos(),
                    }
                    .into());
                }
                if output.pos() < declared {
                    return Err(EnvelopeError::DecodedShort {
                        declared,
                        got: output.pos(),
                    });
                }
                return Ok(());
            }
            if (input.pos(), output.pos()) == before {
                return Err(if output.pos() == declared {
                    EnvelopeError::DecodedLong { declared }
                } else {
                    ProtoError::Truncated.into()
                });
            }
        }
    }
}

/// The window size a standard zstd frame header declares (RFC 8878 3.1.1.1), or `None` when
/// `src` does not start with a complete standard frame header. zstd itself rejects what this
/// cannot read.
fn frame_window_size(src: &[u8]) -> Option<u64> {
    let (magic, rest) = src.split_first_chunk::<4>()?;
    if u32::from_le_bytes(*magic) != ZSTD_MAGIC {
        return None;
    }
    let (&descriptor, rest) = rest.split_first()?;
    if descriptor & 0x20 == 0 {
        // Window descriptor: exponent in the high 5 bits, mantissa in the low 3.
        let &window = rest.first()?;
        let base = 1u64 << (10 + u32::from(window >> 3));
        return Some(base + base / 8 * u64::from(window & 7));
    }
    // Single segment: the window is the frame content size, after the dictionary id.
    let dict_len = [0, 1, 2, 4][usize::from(descriptor & 0x03)];
    let fcs_len = [1, 2, 4, 8][usize::from(descriptor >> 6)];
    let fcs = rest.get(dict_len..dict_len + fcs_len)?;
    let mut b = [0u8; 8];
    b[..fcs_len].copy_from_slice(fcs);
    let size = u64::from_le_bytes(b);
    Some(if fcs_len == 2 { size + 256 } else { size })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use proptest::prelude::*;
    use tl_model::journal::{JournalError, JournalEvent};
    use tl_model::remote_proto::{
        BootPrefix, Hello, HelloTail, StreamId, Want, decode_hello, encode_hello,
    };

    use super::*;

    const MIB: usize = 1024 * 1024;

    fn exit_msg(stderr_len: usize) -> ServerMsg {
        ServerMsg::Journal {
            stream: StreamId(3),
            event: JournalEvent::Error(JournalError::Exit {
                code: 1,
                stderr_first_line: Arc::from("x".repeat(stderr_len)),
            }),
        }
    }

    /// Payload bytes of `msg` before compression.
    fn plain_len(msg: &ServerMsg) -> usize {
        let mut bytes = Vec::new();
        encode_message(msg, &mut bytes).unwrap();
        bytes.len() - HEADER_LEN
    }

    fn encode(msg: &ServerMsg) -> Vec<u8> {
        let mut out = Vec::new();
        FrameWriter::new()
            .unwrap()
            .encode_server(msg, &mut out)
            .unwrap();
        out
    }

    fn one_frame(bytes: &[u8]) -> RawFrame {
        let mut r = bytes;
        let frame = FrameReader::new().unwrap().read_frame(&mut r).unwrap();
        assert!(r.is_empty(), "one frame expected");
        frame
    }

    fn compress(data: &[u8], window_log: Option<u32>) -> Vec<u8> {
        let mut cctx = CCtx::create();
        cctx.set_parameter(CParameter::CompressionLevel(ZSTD_LEVEL))
            .unwrap();
        if let Some(log) = window_log {
            cctx.set_parameter(CParameter::WindowLog(log)).unwrap();
        }
        let mut out = Vec::with_capacity(zstd_safe::compress_bound(data.len()));
        cctx.compress2(&mut out, data).unwrap();
        out
    }

    /// A compressed frame of this proto with the given payload and declared decoded length.
    fn zstd_frame(payload: Vec<u8>, decoded_len: usize) -> RawFrame {
        RawFrame {
            header: FrameHeader {
                proto: PROTO,
                flags: FLAG_ZSTD,
                encoded_len: payload.len() as u32,
                decoded_len: decoded_len as u32,
            },
            payload,
        }
    }

    #[test]
    fn small_messages_stay_plain() {
        let mut out = Vec::new();
        FrameWriter::new()
            .unwrap()
            .encode_client(&ClientMsg::Ack { frame: 7 }, &mut out)
            .unwrap();
        let frame = one_frame(&out);
        assert_eq!(frame.header.flags, 0);
        let msg = FrameReader::new().unwrap().decode_client(&frame).unwrap();
        assert!(matches!(msg, ClientMsg::Ack { frame: 7 }), "{msg:?}");
    }

    #[test]
    fn compression_round_trip() {
        let msg = exit_msg(10_000);
        let bytes = encode(&msg);
        let frame = one_frame(&bytes);
        assert_eq!(frame.header.flags, FLAG_ZSTD);
        assert_eq!(frame.header.decoded_len as usize, plain_len(&msg));
        assert!(frame.header.encoded_len < frame.header.decoded_len);
        let back = FrameReader::new().unwrap().decode_server(&frame).unwrap();
        assert_eq!(encode(&back), bytes);
        assert_eq!(format!("{msg:?}"), format!("{back:?}"));
    }

    #[test]
    fn one_writer_and_reader_handle_many_frames() {
        let mut writer = FrameWriter::new().unwrap();
        let mut out = Vec::new();
        for n in [5_000, 10, 20_000] {
            writer.encode_server(&exit_msg(n), &mut out).unwrap();
        }
        let mut reader = FrameReader::new().unwrap();
        let mut r = &out[..];
        for n in [5_000, 10, 20_000] {
            let frame = reader.read_frame(&mut r).unwrap();
            let back = reader.decode_server(&frame).unwrap();
            assert_eq!(format!("{back:?}"), format!("{:?}", exit_msg(n)));
        }
        assert!(matches!(
            reader.read_frame(&mut r),
            Err(EnvelopeError::Proto(ProtoError::Eof))
        ));
    }

    #[test]
    fn compress_threshold() {
        // The string length prefix is 2 bytes from 128 to 16383, so the payload grows by one
        // byte per character here.
        let n = 1000 + COMPRESS_ABOVE - plain_len(&exit_msg(1000));
        assert_eq!(plain_len(&exit_msg(n)), COMPRESS_ABOVE);
        assert_eq!(one_frame(&encode(&exit_msg(n))).header.flags, 0);
        assert_eq!(plain_len(&exit_msg(n + 1)), COMPRESS_ABOVE + 1);
        assert_eq!(one_frame(&encode(&exit_msg(n + 1))).header.flags, FLAG_ZSTD);
    }

    #[test]
    fn zip_bomb_is_rejected_within_declared_length() {
        let bomb = compress(&vec![0u8; 32 * MIB], None);
        assert!(bomb.len() < 64 * 1024, "{}", bomb.len());
        let frame = zstd_frame(bomb.clone(), 4096);
        let mut reader = FrameReader::new().unwrap();
        assert!(matches!(
            reader.decode_server(&frame),
            Err(EnvelopeError::DecodedLong { declared: 4096 })
        ));
        let mut dst = [0u8; 4096];
        assert!(matches!(
            reader.decompress(&bomb, &mut dst),
            Err(EnvelopeError::DecodedLong { declared: 4096 })
        ));
    }

    #[test]
    fn limits_checked_before_payload() {
        let reader = FrameReader::new().unwrap();
        let header = |encoded: usize, decoded: usize| {
            FrameHeader {
                proto: PROTO,
                flags: FLAG_ZSTD,
                encoded_len: encoded as u32,
                decoded_len: decoded as u32,
            }
            .to_bytes()
        };
        let over = header(10, MAX_FRAME_LEN + 1);
        assert!(matches!(
            reader.read_frame(&mut &over[..]),
            Err(EnvelopeError::Proto(ProtoError::TooLarge { .. }))
        ));
        let capped = header(20 * MIB, 64 * MIB);
        match reader.read_frame(&mut &capped[..]) {
            Err(EnvelopeError::InputCap { len, max }) => {
                assert_eq!((len, max), (84 * MIB, SESSION_INPUT_MAX));
            }
            other => panic!("{other:?}"),
        }
        // At the cap the header passes and the missing payload is the error.
        let at_cap = header(16 * MIB, 64 * MIB);
        assert!(matches!(
            reader.read_frame(&mut &at_cap[..]),
            Err(EnvelopeError::Proto(ProtoError::Truncated))
        ));
        // A plain frame counts its payload once.
        let plain = FrameHeader::plain(PROTO, 64 * MIB as u32).to_bytes();
        assert!(matches!(
            reader.read_frame(&mut &plain[..]),
            Err(EnvelopeError::Proto(ProtoError::Truncated))
        ));
    }

    #[test]
    fn window_over_8_mib_is_rejected() {
        let mut reader = FrameReader::new().unwrap();
        // 12 MiB fits a 16 MiB window, so the frame is single segment with a 12 MiB window.
        let data = vec![7u8; 12 * MIB];
        let wide = compress(&data, Some(24));
        assert_eq!(frame_window_size(&wide), Some(12 * MIB as u64));
        assert!(matches!(
            reader.decompress(&wide, &mut vec![0u8; data.len()]),
            Err(EnvelopeError::WindowTooLarge { .. })
        ));
        // 20 MiB does not, so the frame carries a 16 MiB window descriptor.
        let wider = compress(&vec![7u8; 20 * MIB], Some(24));
        assert_eq!(frame_window_size(&wider), Some(16 * MIB as u64));
        assert!(matches!(
            reader.decompress(&wider, &mut vec![0u8; 20 * MIB]),
            Err(EnvelopeError::WindowTooLarge { .. })
        ));
        // The same data at plain level 1 uses a small window and decodes.
        let narrow = compress(&data, None);
        assert!(frame_window_size(&narrow).unwrap() <= WINDOW_MAX);
        let mut out = vec![0u8; data.len()];
        reader.decompress(&narrow, &mut out).unwrap();
        assert!(out == data);
    }

    #[test]
    fn window_size_from_header() {
        let magic = ZSTD_MAGIC.to_le_bytes();
        // Exponent 13, mantissa 1: 8 MiB + 1 MiB.
        let described = [&magic[..], &[0x00, (13 << 3) | 1]].concat();
        assert_eq!(frame_window_size(&described), Some(9 * MIB as u64));
        // Single segment with a 2-byte content size, which counts from 256.
        let single = [&magic[..], &[0x60, 0x00, 0x01]].concat();
        assert_eq!(frame_window_size(&single), Some(512));
        // Single segment with a 1-byte dictionary id before a 4-byte content size.
        let dict = [&magic[..], &[0xA1, 0x55, 0x00, 0x00, 0x00, 0x02]].concat();
        assert_eq!(frame_window_size(&dict), Some(32 * MIB as u64));
        assert_eq!(frame_window_size(&single[..6]), None);
        assert_eq!(frame_window_size(&magic), None);
        assert_eq!(frame_window_size(&0x184D_2A50u32.to_le_bytes()), None);
    }

    #[test]
    fn short_and_trailing_payloads_are_errors() {
        let msg = exit_msg(10_000);
        let packed = one_frame(&encode(&msg)).payload;
        let decoded = plain_len(&msg);
        let mut reader = FrameReader::new().unwrap();

        let short = zstd_frame(packed[..packed.len() - 1].to_vec(), decoded);
        assert!(matches!(
            reader.decode_server(&short),
            Err(EnvelopeError::Proto(ProtoError::Truncated))
        ));

        let trailing = zstd_frame([&packed[..], b"xyz"].concat(), decoded);
        assert!(matches!(
            reader.decode_server(&trailing),
            Err(EnvelopeError::Proto(ProtoError::TrailingBytes { extra: 3 }))
        ));

        let declared_more = zstd_frame(packed.clone(), decoded + 5);
        match reader.decode_server(&declared_more) {
            Err(EnvelopeError::DecodedShort { declared, got }) => {
                assert_eq!((declared, got), (decoded + 5, decoded));
            }
            other => panic!("{other:?}"),
        }

        let declared_less = zstd_frame(packed.clone(), decoded - 1);
        assert!(matches!(
            reader.decode_server(&declared_less),
            Err(EnvelopeError::DecodedLong { .. })
        ));

        // A complete decode after the errors: the context is reset for each frame.
        let ok = zstd_frame(packed.clone(), decoded);
        assert!(reader.decode_server(&ok).is_ok());

        let mut other_proto = zstd_frame(packed, decoded);
        other_proto.header.proto = PROTO + 1;
        assert!(matches!(
            reader.decode_server(&other_proto),
            Err(EnvelopeError::Proto(ProtoError::ProtoMismatch { .. }))
        ));
    }

    #[test]
    fn compressed_bootstrap_frames_are_refused() {
        let hello = Hello {
            prefix: BootPrefix::ours(),
            tail: Some(HelloTail {
                pid: 1,
                app_id: Arc::from("tasklens"),
                bridge: false,
                want: Want::default(),
            }),
        };
        let mut bytes = Vec::new();
        encode_hello(&hello, &mut bytes).unwrap();
        let mut frame = one_frame(&bytes);
        frame.header.flags = FLAG_ZSTD;
        assert!(matches!(decode_hello(&frame), Err(ProtoError::Compressed)));
    }

    proptest! {
        #[test]
        fn arbitrary_frames_never_panic(
            compressed in any::<bool>(),
            decoded_len in 0usize..8192,
            payload in proptest::collection::vec(any::<u8>(), 0..2048),
        ) {
            let mut bytes = FrameHeader {
                proto: PROTO,
                flags: if compressed { FLAG_ZSTD } else { 0 },
                encoded_len: payload.len() as u32,
                decoded_len: if compressed { decoded_len as u32 } else { payload.len() as u32 },
            }
            .to_bytes()
            .to_vec();
            bytes.extend_from_slice(&payload);
            let mut reader = FrameReader::new().unwrap();
            if let Ok(frame) = reader.read_frame(&mut &bytes[..]) {
                let _ = reader.decode_server(&frame);
                let _ = reader.decode_client(&frame);
            }
        }

        #[test]
        fn damaged_compressed_frames_never_panic(
            edits in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..8),
            decoded_delta in -64i64..64,
        ) {
            let msg = exit_msg(3000);
            let mut frame = one_frame(&encode(&msg));
            for (at, byte) in edits {
                let at = at % frame.payload.len();
                frame.payload[at] = byte;
            }
            let decoded = i64::from(frame.header.decoded_len) + decoded_delta;
            frame.header.decoded_len = decoded.max(0) as u32;
            let mut reader = FrameReader::new().unwrap();
            let _ = reader.decode_server(&frame);
        }
    }
}
